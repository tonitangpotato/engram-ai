//! LongMemEval scorer (design §3.2, §9.2).
//!
//! Answer-correctness judge that consumes one driver-replayed retrieval
//! and produces the structured score that drives `longmemeval_per_query.jsonl`
//! and the `overall` field of `longmemeval_summary.json` (output contracts
//! in design §3.2). The driver itself enriches that summary with the
//! `v02_baseline` and `delta_pp` fields by reading `baselines/v02.toml`
//! per §5.1 — that is **not** this module's responsibility.
//!
//! ## Scope of this task
//!
//! `task:bench-impl-scorer-longmemeval` (single task, not split). Same
//! shape as `task:bench-impl-scorer-locomo` (sub-task 1): types +
//! normalised-exact-match scoring + synthetic unit tests. The
//! upstream-parity test (analogous to `task:bench-impl-scorer-locomo-2`)
//! is NOT in scope for this task — it requires the vendored upstream
//! LongMemEval Python scorer + a 50-query golden file produced by it,
//! per §9.2's "same policy as §9.1". When that artefact is available, a
//! follow-up task will add the parity assertion exactly like its LOCOMO
//! sibling.
//!
//! ## Scoring methodology
//!
//! For this task we implement a normalised exact-match judge:
//!
//! 1. Normalise both predicted and gold answers: lowercase, collapse
//!    runs of ASCII whitespace into single spaces, strip a fixed set of
//!    punctuation characters (`. , ; : ! ? " '`).
//! 2. Compare the normalised forms. Match ⇒ score `1.0`; mismatch ⇒
//!    score `0.0`.
//!
//! This is a deliberately simple, deterministic baseline. The bit-parity
//! test (future follow-up) is the gate that confirms this matches the
//! upstream Python scorer's behaviour on a committed 50-query fixture;
//! if upstream uses richer logic (token-F1, per-question-type rules),
//! the parity test will fail and a follow-up sub-task is responsible for
//! upgrading this implementation. Per design §9.2: "Scoring is
//! LongMemEval's own scorer, vendored identically."
//!
//! ## Determinism
//!
//! No randomness, no allocator-dependent ordering, no system clock. Same
//! input ⇒ byte-identical output (design §3.2 inherits §3.1's
//! determinism contract).
//!
//! ## Why not share code with `scorers::locomo`?
//!
//! Today both scorers happen to use the same normalised-exact-match
//! baseline, but each is a stand-in for an upstream scorer that may
//! differ. LOCOMO and LongMemEval ship independent Python scorers; the
//! parity tests will pin each to its own upstream. Sharing a normalisation
//! helper now would create a structural coupling that we'd have to
//! unpick the moment one upstream demands richer logic. The duplication
//! is intentional and small (~30 lines); it disappears as soon as either
//! upstream demands divergence. See design §9.1 / §9.2 — vendoring is
//! per-suite.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One LongMemEval query with its gold answer + question-type tag
/// (design §3.2 inherits §3.1's per-query JSONL shape:
/// `{id, category, gold, predicted, score, latency_ms}`).
///
/// `category` matches the LongMemEval question-type taxonomy (e.g.
/// `single-session-user`, `multi-session`, `temporal-reasoning`,
/// `knowledge-update`). It is preserved verbatim from the dataset; the
/// scorer aggregates by it but does not interpret it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LongMemEvalQuery {
    /// Query id from the LongMemEval dataset (stable across runs).
    pub id: String,
    /// LongMemEval question-type tag (e.g. `multi-session`).
    pub category: String,
    /// The agent's answer for this query.
    pub predicted: String,
    /// The dataset's gold answer.
    pub gold: String,
}

/// Per-query score record (design §3.2 `longmemeval_per_query.jsonl`
/// shape, same shape as LOCOMO per §3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LongMemEvalScore {
    /// Query id (echoed for join-by-id with the input).
    pub id: String,
    /// Category tag (echoed so downstream consumers can re-aggregate).
    pub category: String,
    /// Normalised predicted answer (the canonical form actually compared).
    pub predicted_normalised: String,
    /// Normalised gold answer.
    pub gold_normalised: String,
    /// Final score in `[0.0, 1.0]`. For this task's exact-match scorer
    /// this is `0.0` or `1.0`; the parity follow-up may upgrade to
    /// fractional scores if the upstream scorer demands it.
    pub score: f64,
}

/// Aggregated summary across a query set.
///
/// This produces the **scorer-owned** half of `longmemeval_summary.json`
/// (`overall`, `by_category`, `n_queries`). The driver (§3.2) extends
/// the on-disk JSON with `v02_baseline` + `delta_pp` from
/// `baselines/v02.toml` per §5.1; those fields live on the driver's own
/// summary type, not here. Keeping the boundary clean means the parity
/// test only has to compare scorer output (no baselines) against the
/// upstream Python scorer's output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LongMemEvalSummary {
    /// Overall mean score across all queries. `0.0` when `n_queries = 0`
    /// (caller must surface that as `RunStatus::Error`, not a silent
    /// zero — per §4.4 Level 1).
    pub overall: f64,
    /// Mean score broken down by question-type. `BTreeMap` for
    /// deterministic JSON serialization order across runs.
    pub by_category: BTreeMap<String, f64>,
    /// Total query count (sanity-check for the meta-gate §4.2a).
    pub n_queries: usize,
}

/// Stateless scorer. Holds no configuration today; constructed via
/// `LongMemEvalScorer::default()` so callers don't have to thread mutable
/// state through their replay loop.
#[derive(Debug, Clone, Default)]
pub struct LongMemEvalScorer;

impl LongMemEvalScorer {
    /// Score a batch of queries and produce both the per-query records
    /// and the aggregated summary (driver writes both to disk in
    /// `longmemeval_per_query.jsonl` + `longmemeval_summary.json`, the
    /// latter extended with baseline fields by the driver).
    pub fn score(&self, queries: &[LongMemEvalQuery]) -> (Vec<LongMemEvalScore>, LongMemEvalSummary) {
        let mut per_query: Vec<LongMemEvalScore> = Vec::with_capacity(queries.len());
        // (sum, count) per category — finalised to mean below.
        let mut by_cat: BTreeMap<String, (f64, usize)> = BTreeMap::new();
        let mut total = 0.0_f64;

        for q in queries {
            let predicted_n = normalise(&q.predicted);
            let gold_n = normalise(&q.gold);
            let score = if predicted_n == gold_n { 1.0 } else { 0.0 };
            total += score;
            let entry = by_cat.entry(q.category.clone()).or_insert((0.0, 0));
            entry.0 += score;
            entry.1 += 1;
            per_query.push(LongMemEvalScore {
                id: q.id.clone(),
                category: q.category.clone(),
                predicted_normalised: predicted_n,
                gold_normalised: gold_n,
                score,
            });
        }

        let overall = if queries.is_empty() {
            0.0
        } else {
            total / queries.len() as f64
        };
        let by_category = by_cat
            .into_iter()
            .map(|(cat, (sum, n))| (cat, sum / n as f64))
            .collect();

        let summary = LongMemEvalSummary {
            overall,
            by_category,
            n_queries: queries.len(),
        };
        (per_query, summary)
    }
}

/// Normalisation rule (lowercase, whitespace-collapse, strip a fixed
/// punctuation set). Documented at module level; exposed `pub(crate)` so
/// a future parity test can pre-normalise its golden file once. Intentionally
/// duplicated from `scorers::locomo::normalise` — see module docs for
/// the rationale (each scorer owns its own normalisation rule because
/// upstream Python scorers may diverge).
pub(crate) fn normalise(text: &str) -> String {
    let mut buf = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.chars() {
        let c = ch.to_ascii_lowercase();
        if matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'') {
            continue;
        }
        if c.is_ascii_whitespace() {
            if !prev_space && !buf.is_empty() {
                buf.push(' ');
                prev_space = true;
            }
            continue;
        }
        buf.push(c);
        prev_space = false;
    }
    // Trim trailing space introduced by the collapse loop.
    while buf.ends_with(' ') {
        buf.pop();
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: &str, cat: &str, predicted: &str, gold: &str) -> LongMemEvalQuery {
        LongMemEvalQuery {
            id: id.into(),
            category: cat.into(),
            predicted: predicted.into(),
            gold: gold.into(),
        }
    }

    /// Test 1: exact match scores `1.0`, mismatch scores `0.0`. The
    /// minimal contract every other test rests on.
    #[test]
    fn exact_match_and_mismatch() {
        let scorer = LongMemEvalScorer;
        let qs = vec![
            q("a", "single-session-user", "Alice", "Alice"),
            q("b", "single-session-user", "Bob", "Alice"),
        ];
        let (per, sum) = scorer.score(&qs);
        assert_eq!(per[0].score, 1.0);
        assert_eq!(per[1].score, 0.0);
        assert_eq!(sum.overall, 0.5);
        assert_eq!(sum.n_queries, 2);
    }

    /// Test 2: normalisation tolerates punctuation, casing, and
    /// whitespace runs. Anchors the documented rule so a future
    /// "improvement" (e.g. dropping case-fold) doesn't silently regress.
    #[test]
    fn normalisation_ignores_case_punctuation_and_whitespace() {
        let scorer = LongMemEvalScorer;
        let qs = vec![q(
            "n1",
            "temporal-reasoning",
            "  On Tuesday,   at 3 PM.  ",
            "on tuesday at 3 pm",
        )];
        let (per, sum) = scorer.score(&qs);
        assert_eq!(per[0].score, 1.0);
        assert_eq!(per[0].predicted_normalised, "on tuesday at 3 pm");
        assert_eq!(sum.overall, 1.0);
    }

    /// Test 3: per-category aggregation reflects exactly the queries in
    /// each bucket (mean within bucket, not over total). Anchors the
    /// per-question-type aggregation that LongMemEval reports use.
    #[test]
    fn by_category_aggregation_is_per_bucket_mean() {
        let scorer = LongMemEvalScorer;
        let qs = vec![
            q("m1", "multi-session", "yes", "yes"), // 1.0
            q("m2", "multi-session", "no", "yes"),  // 0.0
            q("k1", "knowledge-update", "Paris", "Paris"), // 1.0
        ];
        let (_, sum) = scorer.score(&qs);
        assert_eq!(sum.by_category.get("multi-session"), Some(&0.5));
        assert_eq!(sum.by_category.get("knowledge-update"), Some(&1.0));
        // Overall = (1 + 0 + 1) / 3 ≈ 0.6667
        assert!((sum.overall - 2.0 / 3.0).abs() < 1e-12);
    }

    /// Test 4: empty input produces `overall = 0.0` and `n_queries = 0`.
    /// The driver MUST surface this as `RunStatus::Error` per §4.4
    /// Level 1 (missing/null = ERROR, never PASS) — this test pins the
    /// scorer-side contract that an empty input does **not** silently
    /// score `1.0` or `NaN`. Pinning the floor here lets `harness/gates.rs`
    /// rely on `n_queries == 0 ⇒ ERROR-not-PASS`.
    #[test]
    fn empty_input_produces_zero_overall_not_nan() {
        let scorer = LongMemEvalScorer;
        let (per, sum) = scorer.score(&[]);
        assert!(per.is_empty());
        assert_eq!(sum.overall, 0.0);
        assert!(sum.by_category.is_empty());
        assert_eq!(sum.n_queries, 0);
    }
}
