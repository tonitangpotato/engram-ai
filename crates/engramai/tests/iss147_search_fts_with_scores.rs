//! ISS-147 Step 1: contract tests for `Storage::search_fts_with_scores`.
//!
//! `search_fts_with_scores` returns `(MemoryRecord, raw_bm25_score)`
//! tuples where the score is the **positive** BM25 magnitude (SQLite
//! FTS5 `bm25()` is sign-flipped in SQL — larger = better match).
//!
//! These tests pin three contracts the BM25-aware fusion path (the
//! rest of ISS-147) will depend on:
//!
//! 1. Scores are non-negative and monotonically ordered with `rank`.
//! 2. Identifier set + ordering matches `search_fts` exactly on the
//!    same query (the helper differs only by exposing the score).
//! 3. Empty / whitespace queries return an empty vector instead of
//!    erroring (matches existing `search_fts` semantics).

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use tempfile::tempdir;

fn rec(id: &str, content: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 24, 4, 30, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: content.into(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: created,
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.7,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "iss147-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

fn seed(storage: &mut Storage, id: &str, content: &str) {
    let r = rec(id, content);
    storage.add(&r, "default").expect("add memory");
}

/// AC: scores are non-negative and the strongest match comes first.
///
/// We seed three docs where only one literally contains "sweden"
/// (the natural fixture from conv-26 q11 — see ISS-147 §4). That
/// doc must rank #1 with a strictly positive BM25 score; the other
/// docs must not appear in the result set because they don't match.
#[test]
fn iss147_scores_are_positive_and_ranked_for_unified() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("iss147_a.db");

    {
        let mut s = Storage::new(&path).unwrap();
        seed(&mut s, "m-sweden", "necklace from grandma in my home country, Sweden");
        seed(&mut s, "m-coffee", "coffee shop downtown on Tuesday morning");
        seed(&mut s, "m-camping", "camping at the beach last summer with the kids");
    }

    let unified = Storage::with_unified_substrate(&path, true).unwrap();
    let hits = unified
        .search_fts_with_scores("sweden", 10)
        .expect("unified bm25 search");

    assert_eq!(hits.len(), 1, "only the sweden doc should match");
    let (rec, score) = &hits[0];
    assert_eq!(rec.id, "m-sweden");
    assert!(
        *score > 0.0,
        "positive BM25 expected (helper flips SQLite's negative sign), got {score}"
    );
}

/// AC: id set + ordering match `search_fts` exactly on the same
/// query — the new helper differs only by exposing the score.
///
/// This is the contract the fusion-path wiring depends on: callers
/// can drop-in `search_fts_with_scores` wherever `search_fts` is
/// already used without changing recall ordering.
#[test]
fn iss147_parity_with_search_fts_id_order() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("iss147_b.db");

    {
        let mut s = Storage::new(&path).unwrap();
        seed(&mut s, "m-1", "alpha beta gamma");
        seed(&mut s, "m-2", "alpha alpha beta"); // higher tf for alpha
        seed(&mut s, "m-3", "delta epsilon");
    }

    let unified = Storage::with_unified_substrate(&path, true).unwrap();

    let plain: Vec<String> = unified
        .search_fts("alpha", 10)
        .expect("plain")
        .into_iter()
        .map(|r| r.id)
        .collect();
    let scored: Vec<String> = unified
        .search_fts_with_scores("alpha", 10)
        .expect("scored")
        .into_iter()
        .map(|(r, _)| r.id)
        .collect();

    assert_eq!(plain, scored, "id ordering must match search_fts");

    // And scores must be monotonically non-increasing (rank order
    // from SQLite implies bm25() magnitude is non-increasing).
    let scores: Vec<f64> = unified
        .search_fts_with_scores("alpha", 10)
        .expect("scored")
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    for window in scores.windows(2) {
        assert!(
            window[0] >= window[1],
            "scores must be monotonically non-increasing, got {window:?}"
        );
    }
}

/// AC: empty / whitespace-only queries return `Ok(vec![])` not an
/// error. Matches `search_fts` semantics — the BM25 helper must not
/// be more brittle than the plain variant when fusion adapters pass
/// degenerate input.
#[test]
fn iss147_empty_query_returns_empty_vec() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("iss147_c.db");

    {
        let mut s = Storage::new(&path).unwrap();
        seed(&mut s, "m-1", "any old content");
    }

    let unified = Storage::with_unified_substrate(&path, true).unwrap();
    assert!(unified.search_fts_with_scores("", 10).unwrap().is_empty());
    assert!(unified.search_fts_with_scores("   ", 10).unwrap().is_empty());
}

/// AC: legacy (non-unified) path also returns positive scores using
/// `memories_fts`. The two arms of the SQL fork must behave the same
/// w.r.t. score sign and ordering, so benchmark reproducibility is
/// independent of the unified-substrate flag.
#[test]
fn iss147_scores_are_positive_and_ranked_for_legacy() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("iss147_d.db");

    {
        let mut s = Storage::new(&path).unwrap();
        seed(&mut s, "m-sweden", "necklace from grandma in my home country, Sweden");
        seed(&mut s, "m-coffee", "coffee shop downtown on Tuesday morning");
    }

    // Default Storage::new => unified_substrate=false (legacy arm).
    let legacy = Storage::new(&path).unwrap();
    let hits = legacy
        .search_fts_with_scores("sweden", 10)
        .expect("legacy bm25 search");

    assert_eq!(hits.len(), 1);
    let (rec, score) = &hits[0];
    assert_eq!(rec.id, "m-sweden");
    assert!(*score > 0.0, "legacy path must also flip sign, got {score}");
}
