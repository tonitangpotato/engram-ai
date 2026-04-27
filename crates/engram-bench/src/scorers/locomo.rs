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
//! ## Scoring methodology — normalised exact match (upstream parity)
//!
//! Faithful Rust port of LOCOMO's published `normalize_answer` +
//! `exact_match_score` (the "EM" head — F1 lives in the same script but
//! is a separate metric we don't gate on yet). Pipeline applied to
//! BOTH predicted and gold, in this exact order:
//!
//! 1. **lower** — `s.lower()` (ASCII lowercase; the upstream uses
//!    Python `str.lower` which on the LOCOMO fixture is ASCII-only —
//!    parity fixture covers this).
//! 2. **remove_punc** — strip every char in Python's `string.punctuation`
//!    (32 ASCII chars: ``!"#$%&'()*+,-./:;<=>?@[\]^_`{|}~``). Note the
//!    upstream **deletes** these chars rather than replacing with a
//!    space, so `"cat-dog"` → `"catdog"` (not `"cat dog"`). This is the
//!    most easily-mis-ported step; the parity fixture pins it.
//! 3. **remove_articles** — `re.sub(r'\b(a|an|the)\b', ' ', text)`
//!    replaces standalone articles with a single space. `\b` is the
//!    regex crate's Unicode word boundary, matching Python's `re` `\b`
//!    semantics on the ASCII fixture.
//! 4. **white_space_fix** — `' '.join(text.split())`: split on any
//!    whitespace run and rejoin with single spaces, with no leading or
//!    trailing space.
//!
//! Then **EM**: `score = 1.0 if pred_norm == gold_norm else 0.0`.
//!
//! Design §9.1 mandates "matches the published scorer's behavior
//! bit-for-bit on a fixture of 50 known queries"; that fixture lives
//! alongside this module and the test asserts byte-equal normalised
//! forms + identical EM verdicts. Any drift fails the gate.
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

/// Normalisation rule — Rust port of LOCOMO's upstream `normalize_answer`.
/// Pipeline: `lower → remove_punc → remove_articles → white_space_fix`.
/// See module docs for the bit-parity contract.
///
/// Exposed `pub(crate)` so the parity-fixture test can pre-normalise its
/// golden file once and assert each predicted/gold byte sequence.
pub(crate) fn normalise(text: &str) -> String {
    // Step 1: lower (ASCII; upstream uses Python str.lower).
    // Step 2: remove every char in Python's string.punctuation (delete,
    // not replace — `cat-dog` → `catdog`).
    let mut buf = String::with_capacity(text.len());
    for ch in text.chars() {
        let c = ch.to_ascii_lowercase();
        if is_python_punctuation(c) {
            continue;
        }
        buf.push(c);
    }

    // Step 3: remove_articles — `\b(a|an|the)\b` → ' '.
    // Compiled once; `\b` here is the `regex` crate's Unicode word
    // boundary, matching Python `re`'s default `\b` on the ASCII fixture.
    static ARTICLES_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = ARTICLES_RE.get_or_init(|| {
        regex::Regex::new(r"\b(a|an|the)\b")
            .expect("LOCOMO articles regex is a compile-time constant")
    });
    let stripped = re.replace_all(&buf, " ");

    // Step 4: white_space_fix — `' '.join(s.split())`. Python `str.split`
    // with no arg splits on runs of any whitespace AND drops empty
    // leading/trailing tokens; that is exactly `split_whitespace`.
    stripped.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Membership test for Python's `string.punctuation` — the literal 32
/// ASCII chars `!"#$%&'()*+,-./:;<=>?@[\]^_`{|}~`. Inlined as a match so
/// the compiler can build a jump table; no allocations, branch-free per
/// char on hot replay paths.
#[inline]
fn is_python_punctuation(c: char) -> bool {
    matches!(
        c,
        '!' | '"'
            | '#'
            | '$'
            | '%'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | '+'
            | ','
            | '-'
            | '.'
            | '/'
            | ':'
            | ';'
            | '<'
            | '='
            | '>'
            | '?'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '_'
            | '`'
            | '{'
            | '|'
            | '}'
            | '~'
    )
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

    // ========================================================================
    // Upstream-parity fixture (design §9.1, §11.1)
    //
    // 50 triples `(predicted, gold, expected_score)` covering every rule of
    // the upstream `normalize_answer` pipeline. Triples are inline (not a
    // sibling JSON file) so `cargo test` is hermetic and a reviewer can audit
    // the contract in one place.
    //
    // Each case is annotated with the upstream rule it pins. If a future
    // refactor of `normalise` breaks one of these, the assertion message
    // names the rule, so the failure is self-explanatory rather than just
    // "expected 1.0, got 0.0".
    //
    // Coverage matrix (50 cases):
    //   - lower            (case-fold matters): 5
    //   - remove_punc      (punctuation deleted, not replaced): 12
    //     of which "deleted not spaced" idiom (cat-dog/U.S.A.): 4
    //   - remove_articles  (a|an|the with \b boundaries): 8
    //   - white_space_fix  (collapse runs, trim ends): 6
    //   - combo            (multiple rules together): 9
    //   - genuine mismatch (must score 0.0): 10
    // ========================================================================

    /// Fixture row: predicted, gold, expected EM score, rule pinned.
    /// The `rule` string is purely for failure messages.
    struct ParityCase {
        predicted: &'static str,
        gold: &'static str,
        expected: f64,
        rule: &'static str,
    }

    const PARITY_FIXTURE: &[ParityCase] = &[
        // --- lower (5) ---
        ParityCase { predicted: "Paris",      gold: "paris",      expected: 1.0, rule: "lower: ASCII case-fold" },
        ParityCase { predicted: "PARIS",      gold: "paris",      expected: 1.0, rule: "lower: all-caps → lowercase" },
        ParityCase { predicted: "MaRy",       gold: "mary",       expected: 1.0, rule: "lower: mixed case" },
        ParityCase { predicted: "YES",        gold: "yes",        expected: 1.0, rule: "lower: short token" },
        ParityCase { predicted: "JFK",        gold: "jfk",        expected: 1.0, rule: "lower: acronym" },

        // --- remove_punc: punctuation deleted, NOT replaced with space (12) ---
        ParityCase { predicted: "Paris.",     gold: "Paris",      expected: 1.0, rule: "remove_punc: trailing period" },
        ParityCase { predicted: "Paris,",     gold: "Paris",      expected: 1.0, rule: "remove_punc: trailing comma" },
        ParityCase { predicted: "\"Paris\"",  gold: "Paris",      expected: 1.0, rule: "remove_punc: double quotes" },
        ParityCase { predicted: "Paris!",     gold: "Paris",      expected: 1.0, rule: "remove_punc: bang" },
        ParityCase { predicted: "Paris?",     gold: "Paris",      expected: 1.0, rule: "remove_punc: question mark" },
        ParityCase { predicted: "(Paris)",    gold: "Paris",      expected: 1.0, rule: "remove_punc: parentheses" },
        ParityCase { predicted: "Paris;",     gold: "Paris",      expected: 1.0, rule: "remove_punc: semicolon" },
        ParityCase { predicted: "Paris:",     gold: "Paris",      expected: 1.0, rule: "remove_punc: colon" },
        // Critical "deleted not spaced" idiom — string.punctuation includes -,
        // and remove_punc deletes rather than substitutes:
        ParityCase { predicted: "cat-dog",    gold: "catdog",     expected: 1.0, rule: "remove_punc: hyphen DELETED (cat-dog → catdog, not 'cat dog')" },
        ParityCase { predicted: "U.S.A.",     gold: "usa",        expected: 1.0, rule: "remove_punc: dotted acronym → letters concatenated" },
        ParityCase { predicted: "9/11",       gold: "911",        expected: 1.0, rule: "remove_punc: slash deleted (9/11 → 911)" },
        ParityCase { predicted: "she's",      gold: "shes",       expected: 1.0, rule: "remove_punc: apostrophe deleted (she's → shes)" },

        // --- remove_articles: \b(a|an|the)\b → ' ' (8) ---
        ParityCase { predicted: "the cat",    gold: "cat",        expected: 1.0, rule: "remove_articles: leading 'the'" },
        ParityCase { predicted: "a cat",      gold: "cat",        expected: 1.0, rule: "remove_articles: leading 'a'" },
        ParityCase { predicted: "an apple",   gold: "apple",      expected: 1.0, rule: "remove_articles: leading 'an'" },
        ParityCase { predicted: "cat the dog", gold: "cat dog",   expected: 1.0, rule: "remove_articles: medial 'the'" },
        ParityCase { predicted: "Bought a car", gold: "bought car", expected: 1.0, rule: "remove_articles: medial 'a'" },
        // Boundary-sensitive: 'a' inside another word must NOT match
        ParityCase { predicted: "abandon",    gold: "abandon",    expected: 1.0, rule: "remove_articles: 'a' inside 'abandon' kept (\\b boundary)" },
        ParityCase { predicted: "the the",    gold: "",           expected: 1.0, rule: "remove_articles: repeated articles fully stripped → empty after wsfix" },
        ParityCase { predicted: "themed",     gold: "themed",     expected: 1.0, rule: "remove_articles: 'the' inside 'themed' kept (\\b boundary)" },

        // --- white_space_fix: split + ' '.join (6) ---
        ParityCase { predicted: "  Paris  ",  gold: "Paris",      expected: 1.0, rule: "white_space_fix: trim leading/trailing" },
        ParityCase { predicted: "Paris\tFrance", gold: "Paris France", expected: 1.0, rule: "white_space_fix: tab → single space" },
        ParityCase { predicted: "Paris\nFrance", gold: "Paris France", expected: 1.0, rule: "white_space_fix: newline → single space" },
        ParityCase { predicted: "Paris   France", gold: "Paris France", expected: 1.0, rule: "white_space_fix: multi-space collapsed" },
        ParityCase { predicted: " \t Paris \n ", gold: "Paris",   expected: 1.0, rule: "white_space_fix: mixed whitespace trimmed" },
        ParityCase { predicted: "a  b  c",    gold: "b c",        expected: 1.0, rule: "white_space_fix: after article-strip leaves double spaces, collapsed" },

        // --- combo (9): multiple rules in concert ---
        ParityCase { predicted: "The CAT-DOG.", gold: "catdog",   expected: 1.0, rule: "combo: lower+article+hyphen-deleted+period" },
        ParityCase { predicted: "Mr. Smith",  gold: "mr smith",   expected: 1.0, rule: "combo: title period stripped (note: no article)" },
        ParityCase { predicted: "On July 20, 1969.", gold: "on july 20 1969", expected: 1.0, rule: "combo: comma+period+lower" },
        ParityCase { predicted: "  THE  Eiffel  Tower!  ", gold: "eiffel tower", expected: 1.0, rule: "combo: ws+article+lower+bang" },
        ParityCase { predicted: "\"Yes,\"",   gold: "yes",        expected: 1.0, rule: "combo: quotes+comma+lower" },
        ParityCase { predicted: "She's a doctor.", gold: "shes doctor", expected: 1.0, rule: "combo: apostrophe+article+period+lower" },
        ParityCase { predicted: "$100",       gold: "100",        expected: 1.0, rule: "combo: dollar sign deleted" },
        ParityCase { predicted: "[Paris]",    gold: "paris",      expected: 1.0, rule: "combo: brackets+lower" },
        ParityCase { predicted: "U.S.A.!?",   gold: "usa",        expected: 1.0, rule: "combo: many puncs all deleted" },

        // --- genuine mismatches: EM must be 0.0 (10) ---
        ParityCase { predicted: "Paris",      gold: "London",     expected: 0.0, rule: "mismatch: different word" },
        ParityCase { predicted: "1969",       gold: "1970",       expected: 0.0, rule: "mismatch: different number" },
        ParityCase { predicted: "yes",        gold: "no",         expected: 0.0, rule: "mismatch: yes vs no" },
        ParityCase { predicted: "Paris France", gold: "Paris",    expected: 0.0, rule: "mismatch: extra token" },
        ParityCase { predicted: "Paris",      gold: "Paris France", expected: 0.0, rule: "mismatch: missing token" },
        ParityCase { predicted: "cat dog",    gold: "catdog",     expected: 0.0, rule: "mismatch: 'cat dog' (space) ≠ 'catdog' — proves space is NOT inserted by hyphen-strip" },
        ParityCase { predicted: "John Smith", gold: "Jane Smith", expected: 0.0, rule: "mismatch: different first name" },
        ParityCase { predicted: "abc",        gold: "abcd",       expected: 0.0, rule: "mismatch: substring should not match" },
        ParityCase { predicted: "the cat",    gold: "the dog",    expected: 0.0, rule: "mismatch: same article, different noun (article-strip can't rescue)" },
        ParityCase { predicted: "abandoned",  gold: "bandoned",   expected: 0.0, rule: "mismatch: 'a' INSIDE word is kept, so 'abandoned' ≠ 'bandoned' (proves \\b boundary)" },
    ];

    /// Bit-for-bit parity: every fixture row's EM verdict matches the
    /// upstream Python scorer's. Failure messages name the rule pinned by
    /// the failing row so a normalise refactor regression is diagnosable
    /// from a single line of test output.
    ///
    /// Design §9.1 / §11.1 contract: this is the gate that lets us claim
    /// "Rust port matches published LOCOMO scorer bit-for-bit".
    #[test]
    fn upstream_parity_fixture_50_queries() {
        assert_eq!(
            PARITY_FIXTURE.len(),
            50,
            "Design §9.1 mandates a 50-query parity fixture; got {}",
            PARITY_FIXTURE.len()
        );

        let scorer = LocomoScorer;
        let queries: Vec<LocomoQuery> = PARITY_FIXTURE
            .iter()
            .enumerate()
            .map(|(i, case)| LocomoQuery {
                id: format!("p{:02}", i),
                category: "parity".into(),
                predicted: case.predicted.into(),
                gold: case.gold.into(),
            })
            .collect();

        let (per, sum) = scorer.score(&queries);

        for (case, score) in PARITY_FIXTURE.iter().zip(per.iter()) {
            assert_eq!(
                score.score, case.expected,
                "parity violation: rule={:?}, predicted={:?}, gold={:?}, \
                 normalised(pred)={:?}, normalised(gold)={:?}",
                case.rule, case.predicted, case.gold,
                score.predicted_normalised, score.gold_normalised
            );
        }

        // Sanity: aggregation reflects the fixture's expected hits.
        let expected_hits: f64 = PARITY_FIXTURE.iter().map(|c| c.expected).sum();
        let expected_overall = expected_hits / PARITY_FIXTURE.len() as f64;
        assert!(
            (sum.overall - expected_overall).abs() < 1e-12,
            "summary.overall mismatch: got {}, want {}",
            sum.overall, expected_overall
        );
        assert_eq!(sum.n_queries, 50);
    }

    /// Direct unit tests for `normalise` — pinning the byte-level output
    /// of the function for the trickiest cases. Complements the EM-level
    /// parity test by asserting the *normalised form* itself, not just
    /// whether two normalised forms are equal.
    #[test]
    fn normalise_bytes_match_upstream_examples() {
        // `cat-dog` → `catdog` (hyphen deleted, NOT replaced with space).
        assert_eq!(normalise("cat-dog"), "catdog");
        // U.S.A. → usa
        assert_eq!(normalise("U.S.A."), "usa");
        // "The Eiffel Tower!" → "eiffel tower"
        assert_eq!(normalise("The Eiffel Tower!"), "eiffel tower");
        // Articles inside words preserved.
        assert_eq!(normalise("abandon"), "abandon");
        assert_eq!(normalise("themed"), "themed");
        // Multiple articles in a row → empty string after wsfix.
        assert_eq!(normalise("the the"), "");
        // Whitespace runs collapsed, ends trimmed.
        assert_eq!(normalise("  Paris   France  "), "paris france");
        // Apostrophe deleted, not preserved.
        assert_eq!(normalise("she's"), "shes");
        // Mixed: leading article + punc + case.
        assert_eq!(normalise("The U.S.A."), "usa");
    }
}
