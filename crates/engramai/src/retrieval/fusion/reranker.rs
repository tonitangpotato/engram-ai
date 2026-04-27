//! # Reranker contract (`task:retr-impl-reranker-contract`)
//!
//! Optional reranker pass between fusion and the API boundary, per design
//! §5.3. The trait is **the contract** (purity, bounded latency, score
//! preservation, no drops); concrete rerankers (cross-encoders, LLM
//! rerankers) plug in without touching retrieval internals.
//!
//! For v0.3 **no reranker ships by default** — the fusion rule from §5.1/§5.2
//! is the ranking. The trait + a [`NullReranker`] exist so callers can wire
//! one in, and so the property-test contract can be enforced against any
//! implementation.
//!
//! ## Contract (§5.3)
//!
//! 1. **Pure**: given the same `(query, candidates)`, return the same
//!    ordering. No `rand`, no clock, no global mutable state. This is what
//!    makes §5.4 reproducibility work end-to-end.
//! 2. **Bounded latency**: implementations must honor a [`Duration`] budget
//!    or return early with a partial rerank. The trait signature does not
//!    take a budget directly — the executor wraps the call with the
//!    [`crate::retrieval::budget::BudgetController`]; the implementation is
//!    expected to either be fast (<= the configured rerank-stage cap) or to
//!    cooperate with an internal budget passed via its constructor.
//! 3. **Score preservation**: scores in the output MUST be in `[0.0, 1.0]`
//!    and MUST NOT be `NaN`. The reranker may *adjust* scores (that's the
//!    point) but the legal range is fixed.
//! 4. **No drops, only reorder**: the output multiset of records/topics
//!    MUST equal the input multiset. Reranking is permutation + score
//!    adjustment, not filtering.
//!
//! These four properties are enforced by [`assert_reranker_contract`],
//! which any implementation should run in its test suite (the null impl
//! does, see `tests` below).

use std::time::Duration;

use super::super::api::{RetrievalError, ScoredResult};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Optional cross-encoder / LLM reranker, per design §5.3.
///
/// Implementations MUST satisfy:
/// - **Purity** — same `(query, candidates)` → same output.
/// - **Bounded latency** — honor budget or return early.
/// - **Score preservation** — every output score in `[0.0, 1.0]`, no NaN.
/// - **No drops** — output and input have the same multiset of items
///   (only order and scores may change).
///
/// See [`assert_reranker_contract`] for a property-test helper.
pub trait Reranker: Send + Sync {
    /// Rerank `candidates` for `query`. Returns the same set of items in a
    /// (possibly) new order with (possibly) adjusted scores.
    ///
    /// The trait does not take a [`Duration`] budget directly — the
    /// retrieval executor wraps each call with the per-stage budget
    /// controller and discards or accepts partial results from the trace
    /// side. Implementations that internally yield deserve to take the
    /// budget through their constructor.
    fn rerank(
        &self,
        query: &str,
        candidates: &[ScoredResult],
    ) -> Result<Vec<ScoredResult>, RetrievalError>;
}

// ---------------------------------------------------------------------------
// Null implementation (default for v0.3)
// ---------------------------------------------------------------------------

/// Identity reranker: returns the input unchanged. The default for v0.3 —
/// the fusion rule is the ranking unless the caller explicitly wires in a
/// real reranker.
///
/// Trivially satisfies all four contract properties.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullReranker;

impl NullReranker {
    pub const fn new() -> Self {
        Self
    }
}

impl Reranker for NullReranker {
    fn rerank(
        &self,
        _query: &str,
        candidates: &[ScoredResult],
    ) -> Result<Vec<ScoredResult>, RetrievalError> {
        Ok(candidates.to_vec())
    }
}

// ---------------------------------------------------------------------------
// Contract assertion helper
// ---------------------------------------------------------------------------

/// Settings for [`assert_reranker_contract`]. Defaults are reasonable for
/// unit tests; tighten `latency_budget` to verify production rerankers.
#[derive(Debug, Clone)]
pub struct ContractCheck {
    /// Bound on a single rerank call. Default: 1s (loose for unit tests).
    pub latency_budget: Duration,
    /// Number of times to call the reranker on the same input to check
    /// determinism. Default: 3.
    pub determinism_repeats: usize,
}

impl Default for ContractCheck {
    fn default() -> Self {
        Self {
            latency_budget: Duration::from_secs(1),
            determinism_repeats: 3,
        }
    }
}

/// Verify that an implementation satisfies the reranker contract on a given
/// `(query, candidates)` input. Panics with a descriptive message on
/// violation; returns `Ok(())` on success.
///
/// Use this in property tests against any concrete `Reranker`.
///
/// Checks:
/// - **Purity** — running `determinism_repeats` times yields identical
///   output ordering and scores.
/// - **Bounded latency** — each call completes within `latency_budget`.
/// - **Score preservation** — all output scores in `[0.0, 1.0]`, no NaN.
/// - **No drops** — input and output multisets of (variant, id) pairs match.
pub fn assert_reranker_contract<R: Reranker>(
    reranker: &R,
    query: &str,
    candidates: &[ScoredResult],
    cfg: &ContractCheck,
) -> Result<(), String> {
    if cfg.determinism_repeats == 0 {
        return Err("determinism_repeats must be >= 1".to_string());
    }

    // First call: latency + structural checks.
    let start = std::time::Instant::now();
    let first = reranker
        .rerank(query, candidates)
        .map_err(|e| format!("reranker returned error on first call: {e}"))?;
    let elapsed = start.elapsed();
    if elapsed > cfg.latency_budget {
        return Err(format!(
            "latency {elapsed:?} exceeds budget {:?}",
            cfg.latency_budget
        ));
    }

    // Score preservation.
    for (i, c) in first.iter().enumerate() {
        let s = c.score();
        if s.is_nan() {
            return Err(format!("output[{i}] has NaN score"));
        }
        if !(0.0..=1.0).contains(&s) {
            return Err(format!("output[{i}] score {s} not in [0.0, 1.0]"));
        }
    }

    // No drops: multiset equality on (variant, id) keys.
    let want = key_multiset(candidates);
    let got = key_multiset(&first);
    if want != got {
        return Err(format!(
            "output multiset != input multiset (input: {} items, output: {} items)",
            candidates.len(),
            first.len()
        ));
    }

    // Purity: subsequent calls match first byte-for-byte (in our key+score sense).
    for run in 1..cfg.determinism_repeats {
        let next = reranker
            .rerank(query, candidates)
            .map_err(|e| format!("reranker returned error on call {run}: {e}"))?;
        if !same_ordering(&first, &next) {
            return Err(format!(
                "non-deterministic: run 0 and run {run} produced different orderings or scores"
            ));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers (purity / drop checks)
// ---------------------------------------------------------------------------

/// Stable identity for a [`ScoredResult`] used in multiset comparison.
/// Ignores score (which the reranker is allowed to adjust) but preserves
/// variant + identifier so we can detect dropped or substituted items.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum ResultKey {
    Memory(String),
    Topic(String),
}

fn key_of(r: &ScoredResult) -> ResultKey {
    match r {
        ScoredResult::Memory { record, .. } => ResultKey::Memory(record.id.clone()),
        ScoredResult::Topic { topic, .. } => ResultKey::Topic(topic.topic_id.to_string()),
    }
}

fn key_multiset(rs: &[ScoredResult]) -> std::collections::BTreeMap<ResultKey, usize> {
    let mut m = std::collections::BTreeMap::new();
    for r in rs {
        *m.entry(key_of(r)).or_insert(0) += 1;
    }
    m
}

/// True iff two outputs have the same items in the same positions with
/// bit-identical scores. Score equality is bitwise (`to_bits`) because §5.4
/// requires byte-identical results across calls — `==` on `f64` would let
/// `+0.0` and `-0.0` compare equal which is fine, but NaN != NaN would be a
/// false negative; we reject NaN above so bitwise is the strict check.
fn same_ordering(a: &[ScoredResult], b: &[ScoredResult]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).all(|(x, y)| {
        key_of(x) == key_of(y) && x.score().to_bits() == y.score().to_bits()
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::KnowledgeTopic;
    use crate::retrieval::api::SubScores;
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
    use uuid::Uuid;

    // -- fixture builders ---------------------------------------------------

    fn mk_record(id: &str) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: format!("memory-{id}"),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: chrono::Utc::now(),
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

    fn mk_memory(id: &str, score: f64) -> ScoredResult {
        ScoredResult::Memory {
            record: mk_record(id),
            score,
            sub_scores: SubScores::default(),
        }
    }

    fn mk_topic(score: f64) -> ScoredResult {
        ScoredResult::Topic {
            topic: KnowledgeTopic::new(
                Uuid::new_v4(),
                "topic-a".to_string(),
                String::new(),
                "default".to_string(),
                0.0,
            ),
            score,
            source_memories: Vec::new(),
            contributing_entities: Vec::new(),
        }
    }

    fn fixture() -> Vec<ScoredResult> {
        vec![
            mk_memory("m1", 0.9),
            mk_memory("m2", 0.4),
            mk_topic(0.7),
            mk_memory("m3", 0.5),
        ]
    }

    // -- NullReranker ------------------------------------------------------

    #[test]
    fn null_reranker_returns_input_unchanged() {
        let rr = NullReranker::new();
        let input = fixture();
        let out = rr.rerank("anything", &input).unwrap();
        assert_eq!(out.len(), input.len());
        assert!(same_ordering(&input, &out));
    }

    #[test]
    fn null_reranker_satisfies_full_contract() {
        let rr = NullReranker::new();
        let input = fixture();
        let cfg = ContractCheck::default();
        assert_reranker_contract(&rr, "any query", &input, &cfg)
            .expect("null reranker must satisfy contract");
    }

    #[test]
    fn null_reranker_handles_empty_input() {
        let rr = NullReranker::new();
        let out = rr.rerank("q", &[]).unwrap();
        assert!(out.is_empty());
    }

    // -- contract helper: positive cases (deliberately bad rerankers) -----

    /// Drops the first item — violates "no drops".
    struct DroppingReranker;
    impl Reranker for DroppingReranker {
        fn rerank(
            &self,
            _query: &str,
            candidates: &[ScoredResult],
        ) -> Result<Vec<ScoredResult>, RetrievalError> {
            Ok(candidates.iter().skip(1).cloned().collect())
        }
    }

    #[test]
    fn contract_detects_dropped_items() {
        let rr = DroppingReranker;
        let err = assert_reranker_contract(&rr, "q", &fixture(), &ContractCheck::default())
            .expect_err("dropping reranker must fail contract");
        assert!(err.contains("multiset"), "unexpected msg: {err}");
    }

    /// Returns scores outside [0,1] — violates score preservation.
    struct OutOfRangeReranker;
    impl Reranker for OutOfRangeReranker {
        fn rerank(
            &self,
            _query: &str,
            candidates: &[ScoredResult],
        ) -> Result<Vec<ScoredResult>, RetrievalError> {
            let mut out = candidates.to_vec();
            if let Some(first) = out.get_mut(0) {
                match first {
                    ScoredResult::Memory { score, .. } | ScoredResult::Topic { score, .. } => {
                        *score = 1.5;
                    }
                }
            }
            Ok(out)
        }
    }

    #[test]
    fn contract_detects_score_out_of_range() {
        let rr = OutOfRangeReranker;
        let err = assert_reranker_contract(&rr, "q", &fixture(), &ContractCheck::default())
            .expect_err("out-of-range reranker must fail contract");
        assert!(err.contains("not in [0.0, 1.0]"), "unexpected msg: {err}");
    }

    /// Returns NaN — violates score preservation.
    struct NaNReranker;
    impl Reranker for NaNReranker {
        fn rerank(
            &self,
            _query: &str,
            candidates: &[ScoredResult],
        ) -> Result<Vec<ScoredResult>, RetrievalError> {
            let mut out = candidates.to_vec();
            if let Some(first) = out.get_mut(0) {
                match first {
                    ScoredResult::Memory { score, .. } | ScoredResult::Topic { score, .. } => {
                        *score = f64::NAN;
                    }
                }
            }
            Ok(out)
        }
    }

    #[test]
    fn contract_detects_nan_score() {
        let rr = NaNReranker;
        let err = assert_reranker_contract(&rr, "q", &fixture(), &ContractCheck::default())
            .expect_err("NaN reranker must fail contract");
        assert!(err.contains("NaN"), "unexpected msg: {err}");
    }

    /// Non-deterministic: alternates output order based on a counter —
    /// emulated by mutating an internal cell. This verifies the determinism
    /// check catches such implementations.
    struct FlakyReranker {
        counter: std::sync::atomic::AtomicUsize,
    }
    impl Reranker for FlakyReranker {
        fn rerank(
            &self,
            _query: &str,
            candidates: &[ScoredResult],
        ) -> Result<Vec<ScoredResult>, RetrievalError> {
            let n = self
                .counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut out = candidates.to_vec();
            if n % 2 == 1 {
                out.reverse();
            }
            Ok(out)
        }
    }

    #[test]
    fn contract_detects_nondeterminism() {
        let rr = FlakyReranker {
            counter: std::sync::atomic::AtomicUsize::new(0),
        };
        let err = assert_reranker_contract(&rr, "q", &fixture(), &ContractCheck::default())
            .expect_err("flaky reranker must fail contract");
        assert!(err.contains("non-deterministic"), "unexpected msg: {err}");
    }

    /// Always returns Internal error — caught as "returned error".
    struct ErroringReranker;
    impl Reranker for ErroringReranker {
        fn rerank(
            &self,
            _query: &str,
            _candidates: &[ScoredResult],
        ) -> Result<Vec<ScoredResult>, RetrievalError> {
            Err(RetrievalError::Internal("broken".into()))
        }
    }

    #[test]
    fn contract_detects_error_returns() {
        let rr = ErroringReranker;
        let err = assert_reranker_contract(&rr, "q", &fixture(), &ContractCheck::default())
            .expect_err("erroring reranker must fail contract");
        assert!(err.contains("returned error"), "unexpected msg: {err}");
    }

    // -- contract helper: a 'good' reranker that reorders -----------------

    /// Sorts candidates by score descending — pure, fast, score-preserving,
    /// no drops. Should satisfy the contract.
    struct SortByScoreReranker;
    impl Reranker for SortByScoreReranker {
        fn rerank(
            &self,
            _query: &str,
            candidates: &[ScoredResult],
        ) -> Result<Vec<ScoredResult>, RetrievalError> {
            let mut out = candidates.to_vec();
            out.sort_by(|a, b| {
                b.score()
                    .partial_cmp(&a.score())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            Ok(out)
        }
    }

    #[test]
    fn deterministic_reorder_satisfies_contract() {
        let rr = SortByScoreReranker;
        let cfg = ContractCheck::default();
        assert_reranker_contract(&rr, "q", &fixture(), &cfg)
            .expect("deterministic reorder must pass contract");
    }

    #[test]
    fn contract_check_zero_repeats_rejected() {
        let rr = NullReranker::new();
        let cfg = ContractCheck {
            latency_budget: Duration::from_secs(1),
            determinism_repeats: 0,
        };
        let err = assert_reranker_contract(&rr, "q", &fixture(), &cfg)
            .expect_err("zero repeats should be rejected");
        assert!(err.contains("determinism_repeats"));
    }
}
