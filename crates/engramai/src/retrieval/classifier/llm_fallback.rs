//! # Stage-2 LLM fallback (`task:retr-impl-classifier-llm`)
//!
//! Implements the Stage-2 LLM classifier per design §3.2 / §3.4.
//!
//! Entry point is [`run_llm_fallback`], which:
//!
//! 1. Calls the pluggable [`LlmIntentClassifier`] trait under a budget cap
//!    (default 200ms — design §3.2 / §7). Runtime-agnostic: implemented with
//!    a `std::thread` + `mpsc::channel` so callers don't need a Tokio
//!    runtime to invoke the classifier from sync code (the v0.3 retrieval
//!    pipeline is sync-first per design §4 — plans return synchronously).
//! 2. **Validates the LLM's answer against hard signals.** Per §3.2 the
//!    LLM's output is *not trusted blindly* — if the LLM says `Episodic`
//!    when the temporal signal scored `0.0`, that's a *mismatch*; we log it
//!    and fall back to the heuristic best guess (i.e. treat the LLM call as
//!    if it had timed out — the trace still gets a useful provenance label).
//! 3. **Always returns a decision.** Per §3.4 the classifier is total:
//!    timeout / error / mismatch all collapse to "use the heuristic best
//!    guess", with the appropriate `ClassifierMethod` so observability hooks
//!    can tell them apart (GOAL-3.2 — `classifier_method` MUST be
//!    observable; mismatches in particular are surfaced via tracing INFO
//!    per design §8.3).
//!
//! Design refs: §3.2 (two-stage classifier), §3.4 (totality), §7 (budget),
//! §8.1 (`retrieval_classifier_llm_*` metrics — counters bumped by the
//! caller using the returned [`LlmFallbackOutcome`]).

use std::sync::{
    Arc,
    mpsc::{self, RecvTimeoutError},
};
use std::time::Duration;

use super::{ClassifierMethod, DowngradeHint, Intent, Stage1Outcome};
use crate::retrieval::classifier::heuristic::SignalScores;

// ---------------------------------------------------------------------------
// Trait — the pluggable Stage-2 surface
// ---------------------------------------------------------------------------

/// Pluggable LLM-backed intent classifier (Stage 2, §3.2).
///
/// Implementations are expected to be **sync** (the retrieval read path is
/// sync per design §4) and **bounded** in their own latency — the budget
/// cap in [`run_llm_fallback`] is a hard outer guarantee, not a substitute
/// for the implementation respecting its own timeouts. A well-behaved
/// implementation should aim to return well under the configured budget so
/// that the outer thread join doesn't have to forcibly drop work in flight.
///
/// `Send + Sync` is required so a single classifier instance can be wired
/// into the orchestrator behind an `Arc` and shared across query handlers.
pub trait LlmIntentClassifier: Send + Sync {
    /// Classify `query` given the Stage-1 [`SignalScores`].
    ///
    /// Returning `Err` is a recoverable signal — the runner converts it to
    /// `LlmFallbackOutcome::Errored` and the classifier defaults back to the
    /// heuristic best guess (§3.4 totality).
    fn classify(
        &self,
        query: &str,
        signals: &SignalScores,
    ) -> Result<Intent, LlmClassifierError>;
}

/// Recoverable error type from an [`LlmIntentClassifier`].
///
/// All variants collapse to the same fallback behavior (use heuristic best
/// guess, label `ClassifierMethod::HeuristicTimeout`) — they exist as
/// separate variants so metrics / tracing can distinguish them. The names
/// match the `retrieval_classifier_llm_*` metric series in design §8.1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmClassifierError {
    /// Backend reported a transport-level failure (HTTP error, dropped
    /// connection, malformed response, …). The query log records this
    /// without surfacing it to the caller — classifier is total.
    Backend(String),
    /// Backend produced output that did not parse to one of the 5 intents
    /// (e.g., LLM hallucinated a sixth label, returned JSON with the wrong
    /// shape, …). Treated as a recoverable failure per §3.4.
    UnparseableOutput(String),
}

impl std::fmt::Display for LlmClassifierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmClassifierError::Backend(msg) => write!(f, "llm backend error: {msg}"),
            LlmClassifierError::UnparseableOutput(msg) => {
                write!(f, "llm produced unparseable output: {msg}")
            }
        }
    }
}

impl std::error::Error for LlmClassifierError {}

// ---------------------------------------------------------------------------
// Null implementation
// ---------------------------------------------------------------------------

/// `LlmIntentClassifier` that always returns `Backend("llm-disabled")`.
///
/// Wired in by default so the v0.3 retrieval pipeline is **complete and
/// running** even before any real LLM client lands. Per §3.4 the resulting
/// behavior — "always fall back to heuristic best guess" — is correct for
/// production: a system without an LLM client does not get worse than a
/// heuristic-only classifier (it just never reports `LlmFallback` in
/// traces).
///
/// Tests that need a deterministic Stage 2 should provide their own
/// implementation rather than relying on this null.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullLlmClassifier;

impl LlmIntentClassifier for NullLlmClassifier {
    fn classify(
        &self,
        _query: &str,
        _signals: &SignalScores,
    ) -> Result<Intent, LlmClassifierError> {
        Err(LlmClassifierError::Backend("llm-disabled".to_string()))
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Budget / configuration for the Stage-2 runner.
///
/// `budget` is the absolute hard cap on the call (design §3.2: default
/// `200ms`). The runner enforces this regardless of whether the underlying
/// classifier respects its own deadlines — see [`run_llm_fallback`] for the
/// thread-spawning rationale.
#[derive(Debug, Clone, Copy)]
pub struct LlmFallbackConfig {
    pub budget: Duration,
}

impl Default for LlmFallbackConfig {
    fn default() -> Self {
        Self {
            budget: Duration::from_millis(200),
        }
    }
}

// ---------------------------------------------------------------------------
// Outcome — what the runner observed (orchestrator uses this to label trace)
// ---------------------------------------------------------------------------

/// Result of a single Stage-2 invocation.
///
/// The orchestrator (see `task:retr-impl-classifier-core` in
/// `v03-retrieval-build-plan.md`) consumes this to fill in
/// `PlanTrace.classifier.method` (§6.3) and to bump the
/// `retrieval_classifier_llm_*` metric series (§8.1).
#[derive(Debug, Clone, PartialEq)]
pub struct LlmFallbackOutcome {
    /// Final intent + downgrade hint that the orchestrator should use to
    /// build the plan.
    pub intent: Intent,
    pub downgrade_hint: DowngradeHint,
    /// What actually happened — drives `classifier_method` in the trace
    /// (GOAL-3.2 observability).
    pub method: ClassifierMethod,
    /// `true` iff the LLM returned an intent that disagreed with the hard
    /// signals (e.g., `Episodic` when temporal `= 0.0`). Caller logs this
    /// at `INFO` per §8.3 and bumps the
    /// `retrieval_classifier_llm_mismatches_total` metric.
    pub mismatch_detected: bool,
}

// ---------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------

/// Run the Stage-2 LLM fallback for an ambiguous Stage-1 outcome.
///
/// **Preconditions** — `stage1` MUST be [`Stage1Outcome::NeedsLlmFallback`].
/// Calling this on a `Decided` variant is a programmer error; we still
/// behave deterministically (return the decided values back as a
/// `Heuristic`-labelled outcome with no LLM call) so a buggy orchestrator
/// degrades gracefully rather than panicking on the read path.
///
/// **Behavior:**
///
/// - On success **and** signal-consistent answer → `LlmFallback` outcome
///   with the LLM's intent.
/// - On success but **mismatch with hard signals** → `HeuristicTimeout`
///   outcome with the heuristic best guess + `mismatch_detected = true`.
///   We log a tracing `info!` line (visible in `tracing` consumers).
/// - On error → `HeuristicTimeout` outcome with the heuristic best guess.
/// - On budget timeout → `HeuristicTimeout` outcome with the heuristic best
///   guess. The spawned worker thread is detached; the underlying classifier
///   should respect its own deadlines so this case stays rare.
///
/// **Why threads + channel and not `tokio::time::timeout`:** the v0.3
/// retrieval read path is sync (design §4 — plans are pure functions). The
/// classifier may be invoked from contexts without a Tokio runtime
/// (background jobs, CLI tools, FFI consumers). A `std::thread` +
/// `mpsc::channel` keeps the contract runtime-agnostic at the cost of one
/// thread spawn per ambiguous query; given Stage 2 fires only on the
/// "mixed-strength signals" minority of queries (§3.2), the per-call cost
/// is acceptable. If profiling shows otherwise we can swap in a thread
/// pool without changing this signature.
pub fn run_llm_fallback(
    classifier: &Arc<dyn LlmIntentClassifier>,
    query: &str,
    signals: &SignalScores,
    stage1: &Stage1Outcome,
    config: &LlmFallbackConfig,
) -> LlmFallbackOutcome {
    // Extract heuristic best guess + downgrade hint. If Stage-1 already
    // decided, return that decision verbatim (defensive — see doc comment).
    let (heuristic_best_guess, downgrade_hint) = match stage1 {
        Stage1Outcome::NeedsLlmFallback {
            heuristic_best_guess,
            downgrade_hint,
        } => (*heuristic_best_guess, *downgrade_hint),
        Stage1Outcome::Decided {
            intent,
            downgrade_hint,
        } => {
            return LlmFallbackOutcome {
                intent: *intent,
                downgrade_hint: *downgrade_hint,
                method: ClassifierMethod::Heuristic,
                mismatch_detected: false,
            };
        }
    };

    // Spawn the classifier on a worker thread so we can enforce the budget
    // even if the implementation hangs. Clone the Arc so the worker owns
    // its own reference — query/signals are owned by clone too (cheap:
    // String + small struct) so the worker doesn't pin the caller's frame.
    let (tx, rx) = mpsc::channel::<Result<Intent, LlmClassifierError>>();
    let worker_classifier = Arc::clone(classifier);
    let worker_query = query.to_string();
    let worker_signals = signals.clone();

    // Detach the thread (no `join()`) — if the budget fires we abandon it.
    // The worker writes to the channel, which is bounded to one slot's
    // worth of capacity (mpsc is unbounded but we only ever send once);
    // a slow worker that completes after the timeout will find `tx` either
    // alive (caller already returned, channel disconnects on the next send
    // — handled below as a no-op via `let _ = ...send`) or dropped.
    std::thread::spawn(move || {
        let result = worker_classifier.classify(&worker_query, &worker_signals);
        // Ignore send errors: caller may have timed out and dropped `rx`.
        let _ = tx.send(result);
    });

    match rx.recv_timeout(config.budget) {
        Ok(Ok(llm_intent)) => {
            // LLM produced a parse-able intent. Validate against hard signals.
            if signals_contradict_intent(llm_intent, signals) {
                log::info!(
                    target: "retrieval::classifier::llm_fallback",
                    "classifier mismatch: LLM={} heuristic_best_guess={} signals=(e={:.2},t={:.2},a={:.2},af={:.2}); falling back to heuristic best guess",
                    llm_intent.as_str(),
                    heuristic_best_guess.as_str(),
                    signals.entity,
                    signals.temporal,
                    signals.abstract_,
                    signals.affective,
                );
                LlmFallbackOutcome {
                    intent: heuristic_best_guess,
                    downgrade_hint,
                    method: ClassifierMethod::HeuristicTimeout,
                    mismatch_detected: true,
                }
            } else {
                LlmFallbackOutcome {
                    intent: llm_intent,
                    downgrade_hint,
                    method: ClassifierMethod::LlmFallback,
                    mismatch_detected: false,
                }
            }
        }
        Ok(Err(err)) => {
            log::info!(
                target: "retrieval::classifier::llm_fallback",
                "classifier llm error: {} (heuristic_best_guess={}); falling back to heuristic best guess",
                err,
                heuristic_best_guess.as_str(),
            );
            LlmFallbackOutcome {
                intent: heuristic_best_guess,
                downgrade_hint,
                method: ClassifierMethod::HeuristicTimeout,
                mismatch_detected: false,
            }
        }
        Err(RecvTimeoutError::Timeout) => {
            log::info!(
                target: "retrieval::classifier::llm_fallback",
                "classifier llm budget exceeded: budget_ms={} heuristic_best_guess={}; falling back to heuristic best guess",
                config.budget.as_millis() as u64,
                heuristic_best_guess.as_str(),
            );
            LlmFallbackOutcome {
                intent: heuristic_best_guess,
                downgrade_hint,
                method: ClassifierMethod::HeuristicTimeout,
                mismatch_detected: false,
            }
        }
        Err(RecvTimeoutError::Disconnected) => {
            // Worker thread panicked before sending. Same fallback as error.
            log::info!(
                target: "retrieval::classifier::llm_fallback",
                "classifier llm worker disconnected (likely panicked): heuristic_best_guess={}; falling back to heuristic best guess",
                heuristic_best_guess.as_str(),
            );
            LlmFallbackOutcome {
                intent: heuristic_best_guess,
                downgrade_hint,
                method: ClassifierMethod::HeuristicTimeout,
                mismatch_detected: false,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Mismatch validation
// ---------------------------------------------------------------------------

/// `true` iff `llm_intent` is *contradicted* by the hard Stage-1 signals.
///
/// Per design §3.2: "if it returns an intent inconsistent with hard signals
/// (e.g., says `Episodic` when temporal signal was 0.0), the classifier
/// logs a mismatch and defaults back to heuristic's best guess."
///
/// Validation rules — each hard primary intent requires its matching signal
/// to be **strictly positive** (i.e., the heuristic actually fired for that
/// signal). Hybrid requires ≥ 2 strictly-positive primary signals. Factual
/// is permissive (the "default" intent — Stage 1 routes here when no
/// signals strong, so the LLM picking it is never a contradiction).
fn signals_contradict_intent(llm_intent: Intent, signals: &SignalScores) -> bool {
    fn positive(score: f64) -> bool {
        score > 0.0
    }

    match llm_intent {
        // No hard signal required — Factual is the §3.2 default for "no
        // strong signals" so the LLM choosing it is always consistent.
        Intent::Factual => false,
        // Episodic requires the temporal signal to have fired.
        Intent::Episodic => !positive(signals.temporal),
        // Abstract requires the abstract signal to have fired.
        Intent::Abstract => !positive(signals.abstract_),
        // Affective requires the affective signal to have fired.
        Intent::Affective => !positive(signals.affective),
        // Hybrid is "≥ 2 strong signals" (§3.2 / §4.7) — for *contradiction*
        // detection we use the relaxed predicate "≥ 2 positive primary
        // signals". A stricter τ_high check belongs to Stage 1 routing,
        // not the post-hoc LLM mismatch detector — the LLM is allowed to
        // promote a borderline pair to Hybrid; we only catch it when the
        // pair simply doesn't exist.
        Intent::Hybrid => {
            let positive_primaries = signals
                .primary()
                .into_iter()
                .filter(|(_, s)| positive(*s))
                .count();
            positive_primaries < 2
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Test double: returns a pre-programmed intent (or error) after an
    /// optional sleep. `Send + Sync` via interior mutability.
    struct StubClassifier {
        result: Mutex<Result<Intent, LlmClassifierError>>,
        sleep: Duration,
    }

    impl StubClassifier {
        fn ok(intent: Intent) -> Self {
            Self {
                result: Mutex::new(Ok(intent)),
                sleep: Duration::ZERO,
            }
        }

        fn err(err: LlmClassifierError) -> Self {
            Self {
                result: Mutex::new(Err(err)),
                sleep: Duration::ZERO,
            }
        }

        fn slow(intent: Intent, sleep: Duration) -> Self {
            Self {
                result: Mutex::new(Ok(intent)),
                sleep,
            }
        }
    }

    impl LlmIntentClassifier for StubClassifier {
        fn classify(
            &self,
            _query: &str,
            _signals: &SignalScores,
        ) -> Result<Intent, LlmClassifierError> {
            if !self.sleep.is_zero() {
                std::thread::sleep(self.sleep);
            }
            self.result.lock().unwrap().clone()
        }
    }

    fn ambiguous_stage1(best: Intent) -> Stage1Outcome {
        Stage1Outcome::NeedsLlmFallback {
            heuristic_best_guess: best,
            downgrade_hint: DowngradeHint::None,
        }
    }

    fn fast_budget() -> LlmFallbackConfig {
        LlmFallbackConfig {
            budget: Duration::from_millis(200),
        }
    }

    fn arc_classifier(c: impl LlmIntentClassifier + 'static) -> Arc<dyn LlmIntentClassifier> {
        Arc::new(c) as Arc<dyn LlmIntentClassifier>
    }

    // ----- Trait + null impl ------------------------------------------------

    #[test]
    fn null_classifier_returns_disabled_backend_error() {
        let null = NullLlmClassifier;
        let signals = SignalScores::from_primary(0.0, 0.0, 0.0, 0.0);
        let result = null.classify("anything", &signals);
        assert!(matches!(result, Err(LlmClassifierError::Backend(_))));
    }

    #[test]
    fn null_classifier_via_runner_yields_heuristic_timeout() {
        let null = arc_classifier(NullLlmClassifier);
        let signals = SignalScores::from_primary(0.5, 0.0, 0.0, 0.0);
        let stage1 = ambiguous_stage1(Intent::Factual);
        let outcome = run_llm_fallback(&null, "q", &signals, &stage1, &fast_budget());
        assert_eq!(outcome.intent, Intent::Factual);
        assert_eq!(outcome.method, ClassifierMethod::HeuristicTimeout);
        assert!(!outcome.mismatch_detected);
    }

    // ----- Happy path -------------------------------------------------------

    #[test]
    fn signal_consistent_llm_answer_is_used_with_llm_fallback_method() {
        // Temporal signal fired → LLM picking Episodic is consistent.
        let signals = SignalScores::from_primary(0.0, 1.0, 0.0, 0.0);
        let classifier = arc_classifier(StubClassifier::ok(Intent::Episodic));
        let stage1 = ambiguous_stage1(Intent::Factual);
        let outcome = run_llm_fallback(&classifier, "q", &signals, &stage1, &fast_budget());
        assert_eq!(outcome.intent, Intent::Episodic);
        assert_eq!(outcome.method, ClassifierMethod::LlmFallback);
        assert!(!outcome.mismatch_detected);
    }

    #[test]
    fn llm_picking_factual_is_always_consistent() {
        // No signals at all — Factual is the permissive default.
        let signals = SignalScores::from_primary(0.0, 0.0, 0.0, 0.0);
        let classifier = arc_classifier(StubClassifier::ok(Intent::Factual));
        let stage1 = ambiguous_stage1(Intent::Episodic);
        let outcome = run_llm_fallback(&classifier, "q", &signals, &stage1, &fast_budget());
        assert_eq!(outcome.intent, Intent::Factual);
        assert_eq!(outcome.method, ClassifierMethod::LlmFallback);
        assert!(!outcome.mismatch_detected);
    }

    // ----- Mismatch detection ----------------------------------------------

    #[test]
    fn episodic_without_temporal_signal_is_mismatch() {
        // LLM says Episodic but temporal = 0.0 → mismatch.
        let signals = SignalScores::from_primary(0.5, 0.0, 0.0, 0.0);
        let classifier = arc_classifier(StubClassifier::ok(Intent::Episodic));
        let stage1 = ambiguous_stage1(Intent::Factual);
        let outcome = run_llm_fallback(&classifier, "q", &signals, &stage1, &fast_budget());
        assert_eq!(outcome.intent, Intent::Factual); // heuristic best guess
        assert_eq!(outcome.method, ClassifierMethod::HeuristicTimeout);
        assert!(outcome.mismatch_detected);
    }

    #[test]
    fn abstract_without_abstract_signal_is_mismatch() {
        let signals = SignalScores::from_primary(0.5, 0.0, 0.0, 0.0);
        let classifier = arc_classifier(StubClassifier::ok(Intent::Abstract));
        let stage1 = ambiguous_stage1(Intent::Factual);
        let outcome = run_llm_fallback(&classifier, "q", &signals, &stage1, &fast_budget());
        assert!(outcome.mismatch_detected);
        assert_eq!(outcome.intent, Intent::Factual);
        assert_eq!(outcome.method, ClassifierMethod::HeuristicTimeout);
    }

    #[test]
    fn affective_without_affective_signal_is_mismatch() {
        let signals = SignalScores::from_primary(0.5, 0.0, 0.0, 0.0);
        let classifier = arc_classifier(StubClassifier::ok(Intent::Affective));
        let stage1 = ambiguous_stage1(Intent::Factual);
        let outcome = run_llm_fallback(&classifier, "q", &signals, &stage1, &fast_budget());
        assert!(outcome.mismatch_detected);
    }

    #[test]
    fn hybrid_with_only_one_positive_signal_is_mismatch() {
        // Only entity is positive — Hybrid requires ≥2.
        let signals = SignalScores::from_primary(0.5, 0.0, 0.0, 0.0);
        let classifier = arc_classifier(StubClassifier::ok(Intent::Hybrid));
        let stage1 = ambiguous_stage1(Intent::Factual);
        let outcome = run_llm_fallback(&classifier, "q", &signals, &stage1, &fast_budget());
        assert!(outcome.mismatch_detected);
    }

    #[test]
    fn hybrid_with_two_positive_signals_is_consistent() {
        // Entity + temporal both positive (but neither at τ_high) — LLM
        // promoting to Hybrid is allowed at the post-hoc check; the LLM may
        // legitimately combine borderline signals.
        let signals = SignalScores::from_primary(0.5, 0.6, 0.0, 0.0);
        let classifier = arc_classifier(StubClassifier::ok(Intent::Hybrid));
        let stage1 = ambiguous_stage1(Intent::Factual);
        let outcome = run_llm_fallback(&classifier, "q", &signals, &stage1, &fast_budget());
        assert_eq!(outcome.intent, Intent::Hybrid);
        assert_eq!(outcome.method, ClassifierMethod::LlmFallback);
        assert!(!outcome.mismatch_detected);
    }

    // ----- Error handling --------------------------------------------------

    #[test]
    fn backend_error_falls_back_to_heuristic() {
        let signals = SignalScores::from_primary(0.5, 0.0, 0.0, 0.0);
        let classifier = arc_classifier(StubClassifier::err(LlmClassifierError::Backend(
            "boom".to_string(),
        )));
        let stage1 = ambiguous_stage1(Intent::Factual);
        let outcome = run_llm_fallback(&classifier, "q", &signals, &stage1, &fast_budget());
        assert_eq!(outcome.intent, Intent::Factual);
        assert_eq!(outcome.method, ClassifierMethod::HeuristicTimeout);
        assert!(!outcome.mismatch_detected);
    }

    #[test]
    fn unparseable_output_falls_back_to_heuristic() {
        let signals = SignalScores::from_primary(0.5, 0.0, 0.0, 0.0);
        let classifier = arc_classifier(StubClassifier::err(
            LlmClassifierError::UnparseableOutput("not json".to_string()),
        ));
        let stage1 = ambiguous_stage1(Intent::Episodic);
        let outcome = run_llm_fallback(&classifier, "q", &signals, &stage1, &fast_budget());
        assert_eq!(outcome.intent, Intent::Episodic);
        assert_eq!(outcome.method, ClassifierMethod::HeuristicTimeout);
    }

    // ----- Budget enforcement ----------------------------------------------

    #[test]
    fn budget_timeout_falls_back_to_heuristic() {
        // Sleep 500ms but budget is 50ms.
        let signals = SignalScores::from_primary(0.5, 0.0, 0.0, 0.0);
        let classifier = arc_classifier(StubClassifier::slow(
            Intent::Episodic,
            Duration::from_millis(500),
        ));
        let stage1 = ambiguous_stage1(Intent::Factual);
        let cfg = LlmFallbackConfig {
            budget: Duration::from_millis(50),
        };
        let start = std::time::Instant::now();
        let outcome = run_llm_fallback(&classifier, "q", &signals, &stage1, &cfg);
        let elapsed = start.elapsed();

        assert_eq!(outcome.intent, Intent::Factual);
        assert_eq!(outcome.method, ClassifierMethod::HeuristicTimeout);
        assert!(!outcome.mismatch_detected);
        // Should have returned roughly within the budget — generous slack
        // for CI noise but well below the 500ms sleep.
        assert!(
            elapsed < Duration::from_millis(300),
            "runner should respect budget; took {:?}",
            elapsed,
        );
    }

    // ----- Defensive: Stage1::Decided passed in -----------------------------

    #[test]
    fn stage1_decided_passes_through_without_calling_llm() {
        // Programmer-error path: decided Stage 1 should not invoke the LLM.
        // Use a panicking classifier to prove it isn't called.
        struct ExplodingClassifier;
        impl LlmIntentClassifier for ExplodingClassifier {
            fn classify(
                &self,
                _query: &str,
                _signals: &SignalScores,
            ) -> Result<Intent, LlmClassifierError> {
                panic!("classifier invoked despite Stage1::Decided");
            }
        }
        let classifier = arc_classifier(ExplodingClassifier);
        let signals = SignalScores::from_primary(1.0, 0.0, 0.0, 0.0);
        let stage1 = Stage1Outcome::Decided {
            intent: Intent::Factual,
            downgrade_hint: DowngradeHint::None,
        };
        let outcome = run_llm_fallback(&classifier, "q", &signals, &stage1, &fast_budget());
        assert_eq!(outcome.intent, Intent::Factual);
        assert_eq!(outcome.method, ClassifierMethod::Heuristic);
        assert!(!outcome.mismatch_detected);
    }

    // ----- Send + Sync constraint check ------------------------------------

    #[test]
    fn null_classifier_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NullLlmClassifier>();
        assert_send_sync::<Arc<dyn LlmIntentClassifier>>();
    }
}
