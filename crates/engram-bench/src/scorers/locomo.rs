//! LOCOMO scorer (design §3.1, §9.1, §11.1).
//!
//! Answer-equivalence judge that consumes one driver-replayed retrieval
//! and produces the structured score that drives `locomo_summary.json`
//! and `locomo_per_query.jsonl` (output contracts in design §3.1).
//!
//! ## Scope of this sub-task
//!
//! Sub-task 1 of `task:bench-impl-scorer-locomo` — types + scoring math
//! + synthetic unit tests. Sub-task 2 lands the 50-query parity-test
//! fixture that asserts our Rust port matches the upstream Python scorer
//! bit-for-bit (within `1e-6` float tolerance per design §11).
//!
//! ## Scoring methodology
//!
//! LOCOMO ships its own scoring conventions. For sub-task 1 we implement
//! a normalised exact-match judge:
//!
//! 1. Normalise both predicted and gold answers: lowercase, collapse
//!    runs of ASCII whitespace into single spaces, strip a fixed set of
//!    punctuation characters (`. , ; : ! ? " '`).
//! 2. Compare the normalised forms. Match ⇒ score `1.0`; mismatch ⇒
//!    score `0.0`.
//!
//! This is a deliberately simple, deterministic baseline. The bit-parity
//! test (sub-task 2) is the gate that confirms this matches the upstream
//! Python scorer's behaviour on the committed 50-query fixture; if the
//! upstream scorer uses richer logic (token-F1, category-conditional
//! rules), the parity test will fail and sub-task 2 is responsible for
//! upgrading this implementation. Per design §9.1: "matches the published
//! scorer's behavior bit-for-bit on a fixture of 50 known queries".
//!
//! ## Determinism
//!
//! No randomness, no allocator-dependent ordering, no system clock. Same
//! input ⇒ byte-identical output (design §3.1 determinism contract).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One LOCOMO query with its gold answer + category tag (design §3.1
/// per-query JSONL `{id, category, gold, predicted, score, latency_ms}`).
///
/// `category` matches the LOCOMO dataset's category taxonomy (e.g.
/// `temporal`, `single-hop`, `multi-hop`). It is preserved verbatim from
/// the dataset; the scorer aggregates by it but does not interpret it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocomoQuery {
    /// Query id from the LOCOMO dataset (stable across runs).
    pub id: String,
    /// LOCOMO category tag (e.g. `temporal`).
    pub category: String,
    /// The agent's answer for this query.
    pub predicted: String,
    /// The dataset's gold answer.
    pub gold: String,
}

/// Per-query score record (design §3.1 `locomo_per_query.jsonl` shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocomoScore {
    /// Query id (echoed for join-by-id with the input).
    pub id: String,
    /// Category tag (echoed so downstream consumers can re-aggregate).
    pub category: String,
    /// Normalised predicted answer (the canonical form actually compared).
    pub predicted_normalised: String,
    /// Normalised gold answer.
    pub gold_normalised: String,
    /// Final score in `[0.0, 1.0]`. For sub-task 1's exact-match scorer
    /// this is `0.0` or `1.0`; sub-task 2 may upgrade to fractional
    /// scores if the upstream scorer demands it.
    pub score: f64,
}

/// Aggregated summary across a query set (design §3.1
/// `locomo_summary.json` shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocomoSummary {
    /// Overall mean score across all queries. `0.0` when `n_queries = 0`
    /// (caller must surface that as `RunStatus::Error`, not a silent
    /// zero — per §4.4 Level 1).
    pub overall: f64,
    /// Mean score broken down by category. `BTreeMap` for deterministic
    /// JSON serialization order across runs.
    pub by_category: BTreeMap<String, f64>,
    /// Total query count (sanity-check for the meta-gate §4.2a).
    pub n_queries: usize,
}

/// Stateless scorer. Holds no configuration today; constructed via
/// `LocomoScorer::default()` so callers don't have to thread mutable
/// state through their replay loop.
#[derive(Debug, Clone, Default)]
pub struct LocomoScorer;

impl LocomoScorer {
    /// Score a batch of queries and produce both the per-query records
    /// and the aggregated summary (driver writes both to disk in
    /// `locomo_per_query.jsonl` + `locomo_summary.json`).
    pub fn score(&self, queries: &[LocomoQuery]) -> (Vec<LocomoScore>, LocomoSummary) {
        let mut per_query: Vec<LocomoScore> = Vec::with_capacity(queries.len());
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
            per_query.push(LocomoScore {
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

        let summary = LocomoSummary {
            overall,
            by_category,
            n_queries: queries.len(),
        };
        (per_query, summary)
    }
}

/// Normalisation rule (lowercase, whitespace-collapse, strip a fixed
/// punctuation set). Documented at module level; exposed `pub(crate)` so
/// sub-task 2's parity test can pre-normalise its golden file once.
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

    fn q(id: &str, cat: &str, predicted: &str, gold: &str) -> LocomoQuery {
        LocomoQuery {
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
        let scorer = LocomoScorer;
        let qs = vec![
            q("a", "single-hop", "Paris", "Paris"),
            q("b", "single-hop", "London", "Paris"),
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
        let scorer = LocomoScorer;
        let qs = vec![q(
            "n1",
            "temporal",
            "  In 1969,   on July 20.  ",
            "in 1969 on july 20",
        )];
        let (per, sum) = scorer.score(&qs);
        assert_eq!(per[0].score, 1.0);
        assert_eq!(per[0].predicted_normalised, "in 1969 on july 20");
        assert_eq!(sum.overall, 1.0);
    }

    /// Test 3: per-category aggregation reflects exactly the queries in
    /// each bucket (mean within bucket, not over total). Anchors the
    /// `GOAL-5.2 ⇒ by_category.temporal` gate binding from §3.1.
    #[test]
    fn by_category_aggregation_is_per_bucket_mean() {
        let scorer = LocomoScorer;
        let qs = vec![
            q("t1", "temporal", "yes", "yes"),       // 1.0
            q("t2", "temporal", "no", "yes"),        // 0.0
            q("s1", "single-hop", "Paris", "Paris"), // 1.0
        ];
        let (_, sum) = scorer.score(&qs);
        assert_eq!(sum.by_category.get("temporal"), Some(&0.5));
        assert_eq!(sum.by_category.get("single-hop"), Some(&1.0));
        // Overall = (1 + 0 + 1) / 3 ≈ 0.6667
        assert!((sum.overall - 2.0 / 3.0).abs() < 1e-12);
    }
}
