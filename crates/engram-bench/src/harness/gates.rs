//! Gate definitions, registry, and evaluator (design §4).
//!
//! A release gate is a `(goal, metric_key, comparator, threshold)` tuple plus
//! priority semantics (P0 hard / P0 expectation / P1 advisory). This module
//! is the single source of truth for what GOAL-5.x means in numeric terms —
//! drivers MUST NOT inline gate thresholds.
//!
//! Architecture (design §4.3, §4.4):
//!
//! - **Registry** is static and `const`-tied — cannot be mutated at runtime
//!   to silence a failure (design §4.4 Level 3 attestation).
//! - **Drivers** produce a [`MetricSnapshot`] (typed metric values keyed by
//!   `driver.metric_name`) and pass it to [`evaluate_for_driver`].
//! - **Baselines** are resolved through [`BaselineResolver`] so gate eval is
//!   decoupled from the concrete `baselines.rs` types and is unit-testable.
//! - **Missing metrics** map to [`GateStatus::Error`], never to silent pass
//!   — this enforces GUARD-2 (no implicit "not run" → "passed" coercion).
//!
//! GOAL-5.x mapping (authoritative — see
//! `.gid/features/v03-benchmarks/requirements.md` §5):
//!
//! | Goal      | Priority      | Metric key                        | Threshold                       |
//! |-----------|---------------|-----------------------------------|---------------------------------|
//! | GOAL-5.1  | P0Hard        | `locomo.overall`                  | ≥ 0.685                         |
//! | GOAL-5.2  | P0Hard        | `locomo.temporal`                 | ≥ Graphiti baseline             |
//! | GOAL-5.3  | P0Hard        | `longmemeval.delta_pp`            | ≥ 15.0 (percentage points)      |
//! | GOAL-5.4  | P0Hard        | `cost.average_llm_calls`          | ≤ 3.0 (over N=500)              |
//! | GOAL-5.5  | P0Hard        | `tests.v02_pass_rate`             | == 1.0                          |
//! | GOAL-5.6  | P1Advisory    | `cognitive.regression_count`      | == 0                            |
//! | GOAL-5.7  | P1Advisory    | `migration.integrity`             | compound (no loss ∧ ≥20-q parity) |
//! | GOAL-5.8  | P1Advisory    | `repro.complete`                  | compound (record present)       |
//!
//! Notes:
//! - GOAL-5.6 is P1 per requirements.md (not P0Expectation as in earlier drafts).
//! - GOAL-5.7 is P1 per requirements.md; uses a compound metric since the
//!   ship-criterion is "no data loss AND post-migration query parity",
//!   neither of which fits a scalar comparator.
//! - GOAL-5.8 is P2 per requirements.md but mapped to `P1Advisory` since
//!   the registry currently exposes only three priority tiers; advisory
//!   semantics (report, never block) match P2 intent.
//! - There are NO latency gates in the v0.3.0 release contract. Earlier
//!   drafts mapped GOAL-5.3 / 5.7 / 5.8 to `cost.p95_*` keys; those keys
//!   were never produced by any driver and are removed from the registry.
//!   Cost-prefix observability metrics (`cost.average_llm_calls`, etc.)
//!   live on as snapshot metrics — only `cost.average_llm_calls` is a gate.
//!
//! See `.gid/features/v03-benchmarks/design.md §4` for gate-evaluation
//! semantics (compound vs scalar, baseline resolution, missing → Error).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ────────────────────────────────────────────────────────────────────────────
// Comparators, priorities, results
// ────────────────────────────────────────────────────────────────────────────

/// How a metric is compared against its threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Comparator {
    /// metric ≥ threshold (e.g. accuracy floors).
    GreaterOrEqual,
    /// metric ≤ threshold (e.g. latency ceilings).
    LessOrEqual,
    /// metric == threshold (e.g. regression_count == 0).
    Equal,
    /// Multi-condition compound — gate produces its own boolean.
    /// Threshold is ignored; the [`MetricValue::Compound`] variant carries
    /// the verdict.
    Compound,
}

impl Comparator {
    // Back-compat shorthand for callers that used the legacy `Ge`/`Le`/`Eq`
    // names. Prefer the spelled-out variants in new code.
    /// Alias for [`Comparator::GreaterOrEqual`].
    #[allow(non_upper_case_globals)]
    pub const Ge: Comparator = Comparator::GreaterOrEqual;
    /// Alias for [`Comparator::LessOrEqual`].
    #[allow(non_upper_case_globals)]
    pub const Le: Comparator = Comparator::LessOrEqual;
    /// Alias for [`Comparator::Equal`].
    #[allow(non_upper_case_globals)]
    pub const Eq: Comparator = Comparator::Equal;
}

/// Gate priority — controls how a failure is rolled up by the harness.
///
/// See design §4.1 / §4.2 / §4.4 for ship-decision semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Priority {
    /// P0 hard ship gate — failure blocks the release.
    P0Hard,
    /// P0 expectation — expected to hold; a failure requires a documented
    /// override (design §4.4) and surfaces in the release rationale.
    P0Expectation,
    /// P1 advisory — reported but does not block.
    P1Advisory,
}

impl Priority {
    // Back-compat shorthands. The harness aggregator and existing tests
    // still classify outcomes as P0/P1/P2; the registry now distinguishes
    // hard P0 ship gates from P0 expectations, but the rollup tier is the
    // same — both block by default unless overridden.
    /// Legacy alias for [`Priority::P0Hard`].
    pub const P0: Priority = Priority::P0Hard;
    /// Legacy alias — maps to the P0 *expectation* tier (override-eligible).
    /// New code should pick `P0Expectation` or `P1Advisory` explicitly.
    pub const P1: Priority = Priority::P0Expectation;
    /// Legacy alias for [`Priority::P1Advisory`].
    pub const P2: Priority = Priority::P1Advisory;
}

/// Pass / fail / error verdict for one gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GateStatus {
    /// Metric satisfies the threshold under its comparator.
    Pass,
    /// Metric was produced but does not satisfy the threshold.
    Fail,
    /// Metric was missing or the threshold could not be resolved
    /// (e.g. baseline file unavailable). Never coerce to Pass.
    Error,
}

/// Result of evaluating one gate against a snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateResult {
    /// GOAL identifier this result is attributed to (e.g. `"GOAL-5.1"`).
    pub goal: String,
    /// Metric key consulted in the snapshot (e.g. `"locomo.overall"`).
    pub metric_key: String,
    /// Priority tier of this gate — controls release-decision rollup.
    pub priority: Priority,
    /// Comparator that produced the verdict.
    pub comparator: Comparator,
    /// Threshold used at evaluation time, after baseline resolution.
    /// `None` for [`Comparator::Compound`] gates.
    pub threshold: Option<f64>,
    /// Observed value, if produced. `None` for [`Comparator::Compound`]
    /// or when the metric was [`MetricValue::Missing`].
    pub observed: Option<f64>,
    /// Pass / fail / error verdict.
    pub status: GateStatus,
    /// Human-readable explanation. Always populated.
    pub message: String,
}

// ────────────────────────────────────────────────────────────────────────────
// Gate definitions and registry
// ────────────────────────────────────────────────────────────────────────────

/// Threshold variant — either a hard-coded constant (design §4.1) or a value
/// resolved from external baselines at evaluation time (design §4.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Threshold {
    /// Constant threshold pinned in design §4.1.
    Constant(f64),
    /// Baseline-derived threshold; resolved via [`BaselineResolver`].
    /// `description` is the human-readable source (e.g. "Graphiti temporal").
    FromBaseline {
        /// Human-readable source (e.g. `"Graphiti temporal"`).
        description: &'static str,
    },
    /// Compound gate — no scalar threshold; verdict comes from the metric.
    Compound,
}

/// Static gate definition — one row of the design §4 table.
#[derive(Debug, Clone, Copy)]
pub struct GateDefinition {
    /// GOAL identifier (e.g. `"GOAL-5.1"`).
    pub goal: &'static str,
    /// Snapshot key the gate consults (e.g. `"locomo.overall"`).
    pub metric_key: &'static str,
    /// Comparator applied to the (observed, threshold) pair.
    pub comparator: Comparator,
    /// How the threshold is sourced (constant, baseline, compound).
    pub threshold: Threshold,
    /// Priority tier.
    pub priority: Priority,
    /// Human-readable description for reports.
    pub description: &'static str,
}

/// The canonical gate registry. **Static and immutable** by design (§4.4
/// Level 3): runtime mutation would let a release silently weaken a gate.
///
/// Order is significant for reporting only — evaluation is independent
/// per gate. Aggregation order (DAG) is owned by `aggregate_release_decision`
/// in `harness/mod.rs`, not this module.
pub const REGISTRY: &[GateDefinition] = &[
    // ── P0 hard ship gates (requirements.md §5, GOAL-5.1..5.5) ───────────
    GateDefinition {
        goal: "GOAL-5.1",
        metric_key: "locomo.overall",
        comparator: Comparator::GreaterOrEqual,
        threshold: Threshold::Constant(0.685),
        priority: Priority::P0Hard,
        description: "LOCOMO overall accuracy ≥ 68.5% (fraction in [0, 1])",
    },
    GateDefinition {
        goal: "GOAL-5.2",
        metric_key: "locomo.temporal",
        comparator: Comparator::GreaterOrEqual,
        threshold: Threshold::FromBaseline {
            description: "Graphiti temporal",
        },
        priority: Priority::P0Hard,
        description: "LOCOMO temporal ≥ Graphiti baseline",
    },
    GateDefinition {
        goal: "GOAL-5.3",
        metric_key: "longmemeval.delta_pp",
        comparator: Comparator::GreaterOrEqual,
        threshold: Threshold::Constant(15.0),
        priority: Priority::P0Hard,
        description: "LongMemEval ≥ v0.2 baseline + 15 percentage points",
    },
    GateDefinition {
        goal: "GOAL-5.4",
        metric_key: "cost.average_llm_calls",
        comparator: Comparator::LessOrEqual,
        threshold: Threshold::Constant(3.0),
        priority: Priority::P0Hard,
        description: "Average LLM calls per episode ≤ 3 over N=500 run",
    },
    GateDefinition {
        goal: "GOAL-5.5",
        metric_key: "tests.v02_pass_rate",
        comparator: Comparator::Equal,
        threshold: Threshold::Constant(1.0),
        priority: Priority::P0Hard,
        description: "v0.2 test suite 100% pass rate against migrated v0.3 build",
    },
    // ── P1 advisory gates (requirements.md §5, GOAL-5.6..5.8) ────────────
    GateDefinition {
        goal: "GOAL-5.6",
        metric_key: "cognitive.regression_count",
        comparator: Comparator::Equal,
        threshold: Threshold::Constant(0.0),
        priority: Priority::P1Advisory,
        description:
            "No cognitive-property regressions (interoception/metacognition/affect) vs v0.2",
    },
    GateDefinition {
        goal: "GOAL-5.7",
        metric_key: "migration.integrity",
        comparator: Comparator::Compound,
        threshold: Threshold::Compound,
        priority: Priority::P1Advisory,
        description:
            "Migration of rustclaw production DB: no data loss AND ≥20-query parity post-migration",
    },
    GateDefinition {
        goal: "GOAL-5.8",
        metric_key: "repro.complete",
        comparator: Comparator::Compound,
        threshold: Threshold::Compound,
        priority: Priority::P1Advisory,
        description:
            "Reproducibility record committed (commit SHA, dataset versions, weights, raw scores)",
    },
];

/// Borrow the canonical registry. Convenience wrapper for callers that
/// would otherwise reach for `harness::gates::REGISTRY` directly.
pub fn registry() -> &'static [GateDefinition] {
    REGISTRY
}

// ────────────────────────────────────────────────────────────────────────────
// Metric snapshot
// ────────────────────────────────────────────────────────────────────────────

/// One metric value, as produced by a driver.
#[derive(Debug, Clone, PartialEq)]
pub enum MetricValue {
    /// A scalar measurement (accuracy %, latency ms, count).
    Number(f64),
    /// A compound verdict — used for gates whose pass-condition is not
    /// expressible as a single comparator (GOAL-5.5).
    Compound {
        /// Whether the compound condition is satisfied.
        ok: bool,
        /// Human-readable explanation of the verdict.
        message: String,
    },
    /// Metric was not produced. The string explains *why* (fixture missing,
    /// scorer crashed, driver opted out). Maps to [`GateStatus::Error`].
    Missing(String),
}

/// Driver-produced map from metric key → value. Keys follow the convention
/// `<driver>.<metric>` (e.g. `"locomo.overall"`, `"cost.average_llm_calls"`).
#[derive(Debug, Clone, Default)]
pub struct MetricSnapshot {
    metrics: HashMap<String, MetricValue>,
}

impl MetricSnapshot {
    /// Create an empty snapshot.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a scalar metric.
    pub fn set_number(&mut self, key: impl Into<String>, value: f64) {
        self.metrics.insert(key.into(), MetricValue::Number(value));
    }

    /// Insert a compound verdict (for GOAL-5.5 and similar).
    pub fn set_compound(
        &mut self,
        key: impl Into<String>,
        ok: bool,
        message: impl Into<String>,
    ) {
        self.metrics.insert(
            key.into(),
            MetricValue::Compound {
                ok,
                message: message.into(),
            },
        );
    }

    /// Mark a metric as not produced. The reason is preserved in the
    /// resulting [`GateResult::message`].
    pub fn set_missing(&mut self, key: impl Into<String>, reason: impl Into<String>) {
        self.metrics
            .insert(key.into(), MetricValue::Missing(reason.into()));
    }

    /// Borrow the value for `key`, if present.
    pub fn get(&self, key: &str) -> Option<&MetricValue> {
        self.metrics.get(key)
    }

    /// Number of metrics in this snapshot.
    pub fn len(&self) -> usize {
        self.metrics.len()
    }

    /// Returns true if the snapshot has no metrics.
    pub fn is_empty(&self) -> bool {
        self.metrics.is_empty()
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Baseline resolver
// ────────────────────────────────────────────────────────────────────────────

/// Resolves [`Threshold::FromBaseline`] thresholds at evaluation time.
///
/// The benchmark CLI implements this against the loaded `ExternalBaselines`
/// + `V02Baseline`; tests use a stub map. Returning `Err` causes the gate
/// to evaluate to [`GateStatus::Error`] — never to silent pass.
pub trait BaselineResolver {
    /// Resolve `gate`'s baseline-derived threshold to a concrete value.
    /// Returning `Err` causes the gate to evaluate to `GateStatus::Error`.
    fn resolve(&self, gate: &GateDefinition) -> Result<f64, String>;
}

/// A resolver that fails every lookup — convenient for evaluating gates
/// that should not have any baseline-derived thresholds (e.g. unit tests
/// targeting only `Constant` gates).
pub struct NoBaselines;

impl BaselineResolver for NoBaselines {
    fn resolve(&self, gate: &GateDefinition) -> Result<f64, String> {
        Err(format!(
            "no baseline available for {} ({})",
            gate.goal, gate.metric_key
        ))
    }
}

/// In-memory baseline resolver keyed by `goal` (e.g. `"GOAL-5.2"`).
/// Primarily for tests; production wires a custom impl that reads
/// `baselines.rs` types.
#[derive(Debug, Default, Clone)]
pub struct StaticBaselines {
    /// Map from gate goal id (e.g. `"GOAL-5.2"`) to resolved threshold.
    pub by_goal: HashMap<String, f64>,
}

impl StaticBaselines {
    /// Empty resolver — every lookup will fail.
    pub fn new() -> Self {
        Self::default()
    }
    /// Builder-style insert: register `value` as the threshold for `goal`.
    pub fn with(mut self, goal: impl Into<String>, value: f64) -> Self {
        self.by_goal.insert(goal.into(), value);
        self
    }
}

impl BaselineResolver for StaticBaselines {
    fn resolve(&self, gate: &GateDefinition) -> Result<f64, String> {
        self.by_goal
            .get(gate.goal)
            .copied()
            .ok_or_else(|| format!("no baseline registered for {}", gate.goal))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Evaluation
// ────────────────────────────────────────────────────────────────────────────

/// Evaluate one gate against a snapshot.
///
/// Decision order:
///
/// 1. Resolve the threshold (constant, baseline, or compound).
/// 2. Look up the metric — missing → [`GateStatus::Error`].
/// 3. Apply the comparator → [`GateStatus::Pass`] / [`GateStatus::Fail`].
///
/// `Compound` gates short-circuit at step 1: their verdict comes directly
/// from [`MetricValue::Compound`].
pub fn evaluate_gate(
    def: &GateDefinition,
    snapshot: &MetricSnapshot,
    baselines: &dyn BaselineResolver,
) -> GateResult {
    // Compound gates: the metric carries the verdict.
    if matches!(def.comparator, Comparator::Compound) {
        return match snapshot.get(def.metric_key) {
            Some(MetricValue::Compound { ok, message }) => GateResult {
                goal: def.goal.to_string(),
                metric_key: def.metric_key.to_string(),
                priority: def.priority,
                comparator: def.comparator,
                threshold: None,
                observed: None,
                status: if *ok {
                    GateStatus::Pass
                } else {
                    GateStatus::Fail
                },
                message: message.clone(),
            },
            Some(MetricValue::Missing(reason)) => error_result(def, None, reason.clone()),
            Some(other) => error_result(
                def,
                None,
                format!(
                    "compound gate expects MetricValue::Compound, got {:?}",
                    other
                ),
            ),
            None => error_result(
                def,
                None,
                format!("metric '{}' not present in snapshot", def.metric_key),
            ),
        };
    }

    // Resolve threshold for scalar comparators.
    let threshold = match def.threshold {
        Threshold::Constant(v) => v,
        Threshold::FromBaseline { description } => match baselines.resolve(def) {
            Ok(v) => v,
            Err(why) => {
                return error_result(
                    def,
                    None,
                    format!("baseline '{}' unavailable: {}", description, why),
                );
            }
        },
        Threshold::Compound => {
            // Defensive: registry shouldn't pair Compound threshold with a
            // non-Compound comparator, but handle gracefully.
            return error_result(
                def,
                None,
                "registry inconsistency: Compound threshold without Compound comparator"
                    .to_string(),
            );
        }
    };

    // Look up observed metric.
    let observed = match snapshot.get(def.metric_key) {
        Some(MetricValue::Number(v)) => *v,
        Some(MetricValue::Missing(reason)) => {
            return error_result(def, Some(threshold), reason.clone());
        }
        Some(MetricValue::Compound { .. }) => {
            return error_result(
                def,
                Some(threshold),
                format!(
                    "scalar gate expects MetricValue::Number, got Compound for '{}'",
                    def.metric_key
                ),
            );
        }
        None => {
            return error_result(
                def,
                Some(threshold),
                format!("metric '{}' not present in snapshot", def.metric_key),
            );
        }
    };

    let pass = match def.comparator {
        Comparator::GreaterOrEqual => observed >= threshold,
        Comparator::LessOrEqual => observed <= threshold,
        Comparator::Equal => (observed - threshold).abs() < f64::EPSILON,
        Comparator::Compound => unreachable!("handled above"),
    };

    let status = if pass {
        GateStatus::Pass
    } else {
        GateStatus::Fail
    };
    let message = format_scalar_message(def, observed, threshold, status);

    GateResult {
        goal: def.goal.to_string(),
        metric_key: def.metric_key.to_string(),
        priority: def.priority,
        comparator: def.comparator,
        threshold: Some(threshold),
        observed: Some(observed),
        status,
        message,
    }
}

/// Evaluate every gate whose `metric_key` starts with `driver_prefix`
/// (e.g. `"locomo."` or `"cost."`). Returns results in registry order.
///
/// Drivers call this from their `BenchDriver::run` after building a
/// [`MetricSnapshot`] from their internal summary.
pub fn evaluate_for_driver(
    driver_prefix: &str,
    snapshot: &MetricSnapshot,
    baselines: &dyn BaselineResolver,
) -> Vec<GateResult> {
    REGISTRY
        .iter()
        .filter(|def| def.metric_key.starts_with(driver_prefix))
        .map(|def| evaluate_gate(def, snapshot, baselines))
        .collect()
}

/// Evaluate the **entire** registry against a single snapshot. Useful when
/// the harness has aggregated metrics from all drivers into one map (e.g.
/// for the final release decision).
pub fn evaluate_all(
    snapshot: &MetricSnapshot,
    baselines: &dyn BaselineResolver,
) -> Vec<GateResult> {
    REGISTRY
        .iter()
        .map(|def| evaluate_gate(def, snapshot, baselines))
        .collect()
}

// ────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ────────────────────────────────────────────────────────────────────────────

fn error_result(def: &GateDefinition, threshold: Option<f64>, message: String) -> GateResult {
    GateResult {
        goal: def.goal.to_string(),
        metric_key: def.metric_key.to_string(),
        priority: def.priority,
        comparator: def.comparator,
        threshold,
        observed: None,
        status: GateStatus::Error,
        message,
    }
}

fn format_scalar_message(
    def: &GateDefinition,
    observed: f64,
    threshold: f64,
    status: GateStatus,
) -> String {
    let op = match def.comparator {
        Comparator::GreaterOrEqual => "≥",
        Comparator::LessOrEqual => "≤",
        Comparator::Equal => "==",
        Comparator::Compound => "(compound)",
    };
    let verdict = match status {
        GateStatus::Pass => "PASS",
        GateStatus::Fail => "FAIL",
        GateStatus::Error => "ERROR",
    };
    format!(
        "{} [{}] {}: observed={:.4} {} threshold={:.4}",
        verdict, def.goal, def.description, observed, op, threshold
    )
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Registry sanity ────────────────────────────────────────────────

    #[test]
    fn registry_covers_all_goals_5_1_through_5_8() {
        let goals: Vec<_> = REGISTRY.iter().map(|g| g.goal).collect();
        for n in 1..=8 {
            let g = format!("GOAL-5.{}", n);
            assert!(
                goals.contains(&g.as_str()),
                "registry missing {}; have {:?}",
                g,
                goals
            );
        }
    }

    #[test]
    fn registry_metric_keys_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for def in REGISTRY {
            assert!(
                seen.insert(def.metric_key),
                "duplicate metric_key in registry: {}",
                def.metric_key
            );
        }
    }

    #[test]
    fn registry_compound_threshold_pairs_with_compound_comparator() {
        for def in REGISTRY {
            let threshold_compound = matches!(def.threshold, Threshold::Compound);
            let comparator_compound = matches!(def.comparator, Comparator::Compound);
            assert_eq!(
                threshold_compound, comparator_compound,
                "registry inconsistency at {}: Threshold::Compound iff Comparator::Compound",
                def.goal
            );
        }
    }

    // ── Scalar gates ───────────────────────────────────────────────────

    #[test]
    fn ge_gate_passes_when_observed_meets_threshold() {
        let def = find("GOAL-5.1");
        let mut snap = MetricSnapshot::new();
        snap.set_number("locomo.overall", 0.685);
        let r = evaluate_gate(def, &snap, &NoBaselines);
        assert_eq!(r.status, GateStatus::Pass, "{}", r.message);
    }

    #[test]
    fn ge_gate_fails_when_observed_below_threshold() {
        let def = find("GOAL-5.1");
        let mut snap = MetricSnapshot::new();
        snap.set_number("locomo.overall", 0.6849);
        let r = evaluate_gate(def, &snap, &NoBaselines);
        assert_eq!(r.status, GateStatus::Fail);
    }

    #[test]
    fn le_gate_fails_when_observed_above_threshold() {
        // GOAL-5.4: cost.average_llm_calls ≤ 3.0
        let def = find("GOAL-5.4");
        let mut snap = MetricSnapshot::new();
        snap.set_number("cost.average_llm_calls", 3.01);
        let r = evaluate_gate(def, &snap, &NoBaselines);
        assert_eq!(r.status, GateStatus::Fail);
    }

    #[test]
    fn equal_gate_passes_for_zero_regressions() {
        // GOAL-5.6: cognitive.regression_count == 0
        let def = find("GOAL-5.6");
        let mut snap = MetricSnapshot::new();
        snap.set_number("cognitive.regression_count", 0.0);
        let r = evaluate_gate(def, &snap, &NoBaselines);
        assert_eq!(r.status, GateStatus::Pass);
    }

    #[test]
    fn equal_gate_fails_for_any_regression() {
        // GOAL-5.6: cognitive.regression_count == 0
        let def = find("GOAL-5.6");
        let mut snap = MetricSnapshot::new();
        snap.set_number("cognitive.regression_count", 1.0);
        let r = evaluate_gate(def, &snap, &NoBaselines);
        assert_eq!(r.status, GateStatus::Fail);
    }

    // ── Compound gate (GOAL-5.7 / 5.8) ─────────────────────────────────

    #[test]
    fn compound_gate_passes_when_metric_ok() {
        // GOAL-5.7: migration.integrity compound
        let def = find("GOAL-5.7");
        let mut snap = MetricSnapshot::new();
        snap.set_compound(
            "migration.integrity",
            true,
            "no MemoryRecord/Hebbian/topic loss; 22/22 query-set parity",
        );
        let r = evaluate_gate(def, &snap, &NoBaselines);
        assert_eq!(r.status, GateStatus::Pass);
        assert!(r.threshold.is_none());
        assert!(r.observed.is_none());
    }

    #[test]
    fn compound_gate_fails_when_metric_not_ok() {
        let def = find("GOAL-5.7");
        let mut snap = MetricSnapshot::new();
        snap.set_compound("migration.integrity", false, "3 records lost during backfill");
        let r = evaluate_gate(def, &snap, &NoBaselines);
        assert_eq!(r.status, GateStatus::Fail);
        assert!(r.message.contains("3 records lost during backfill"));
    }

    #[test]
    fn compound_gate_errors_when_metric_is_scalar() {
        let def = find("GOAL-5.7");
        let mut snap = MetricSnapshot::new();
        snap.set_number("migration.integrity", 1.0);
        let r = evaluate_gate(def, &snap, &NoBaselines);
        assert_eq!(r.status, GateStatus::Error);
    }

    // ── Missing metrics → Error (GUARD-2) ──────────────────────────────

    #[test]
    fn missing_metric_is_error_not_pass() {
        let def = find("GOAL-5.1");
        let snap = MetricSnapshot::new();
        let r = evaluate_gate(def, &snap, &NoBaselines);
        assert_eq!(
            r.status,
            GateStatus::Error,
            "missing metric must NEVER coerce to Pass (GUARD-2)"
        );
    }

    #[test]
    fn explicitly_missing_metric_preserves_reason() {
        let def = find("GOAL-5.1");
        let mut snap = MetricSnapshot::new();
        snap.set_missing("locomo.overall", "scorer crashed on conv-7");
        let r = evaluate_gate(def, &snap, &NoBaselines);
        assert_eq!(r.status, GateStatus::Error);
        assert!(r.message.contains("scorer crashed on conv-7"));
    }

    // ── Baseline resolution ────────────────────────────────────────────

    #[test]
    fn baseline_gate_passes_when_resolved_threshold_met() {
        let def = find("GOAL-5.2");
        let mut snap = MetricSnapshot::new();
        snap.set_number("locomo.temporal", 60.0);
        let baselines = StaticBaselines::new().with("GOAL-5.2", 55.0);
        let r = evaluate_gate(def, &snap, &baselines);
        assert_eq!(r.status, GateStatus::Pass);
        assert_eq!(r.threshold, Some(55.0));
    }

    #[test]
    fn baseline_gate_errors_when_baseline_missing() {
        let def = find("GOAL-5.2");
        let mut snap = MetricSnapshot::new();
        snap.set_number("locomo.temporal", 60.0);
        let r = evaluate_gate(def, &snap, &NoBaselines);
        assert_eq!(r.status, GateStatus::Error);
        assert!(r.message.to_lowercase().contains("baseline"));
    }

    // ── Driver-scoped evaluation ───────────────────────────────────────

    #[test]
    fn evaluate_for_driver_filters_by_prefix() {
        let mut snap = MetricSnapshot::new();
        snap.set_number("locomo.overall", 70.0);
        snap.set_number("locomo.temporal", 60.0);
        let baselines = StaticBaselines::new().with("GOAL-5.2", 55.0);
        let results = evaluate_for_driver("locomo.", &snap, &baselines);
        let goals: Vec<_> = results.iter().map(|r| r.goal.as_str()).collect();
        assert_eq!(goals, vec!["GOAL-5.1", "GOAL-5.2"]);
        assert!(results.iter().all(|r| r.status == GateStatus::Pass));
    }

    #[test]
    fn evaluate_for_driver_returns_empty_on_unknown_prefix() {
        let snap = MetricSnapshot::new();
        let results = evaluate_for_driver("zzz.", &snap, &NoBaselines);
        assert!(results.is_empty());
    }

    // ── evaluate_all returns one result per registry row ───────────────

    #[test]
    fn evaluate_all_yields_one_result_per_gate() {
        let snap = MetricSnapshot::new();
        let results = evaluate_all(&snap, &NoBaselines);
        assert_eq!(results.len(), REGISTRY.len());
        // Every result is Error because nothing is populated.
        assert!(results.iter().all(|r| r.status == GateStatus::Error));
    }

    // ── Helpers ────────────────────────────────────────────────────────

    fn find(goal: &str) -> &'static GateDefinition {
        REGISTRY
            .iter()
            .find(|g| g.goal == goal)
            .unwrap_or_else(|| panic!("registry missing {}", goal))
    }
}
