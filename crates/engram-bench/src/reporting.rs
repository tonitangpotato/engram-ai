//! Output reporting — summary table, per-gate drill-down, regression
//! alerting (design §10).
//!
//! Three rendering surfaces, all emit to stdout/stderr as plain text
//! (with optional ANSI color when stdout is a TTY); none of them
//! mutate global state.
//!
//! - [`render_summary_table`] — design §10.1. Single-line gate status
//!   per driver, grouped by priority tier (P0 hard, P0 expectation,
//!   P1 advisory). Used by every driver and by `release-gate`.
//! - [`render_drilldown`] — design §10.2. Per-gate explanation:
//!   metric definition, threshold, comparator, raw measurement,
//!   delta vs prior run. Used by `engram-bench explain GOAL-X.Y`.
//! - [`render_diff`] — design §10.3. Regression-alert payload:
//!   gates-changed-status table between two runs.
//!
//! ## Determinism
//!
//! Every renderer is a pure function of its inputs. No system clock,
//! no PRNG, no ENV reads. The same `RunReport` always produces the
//! same string. (Color codes are toggled via an explicit `use_color`
//! parameter — TTY detection is the caller's responsibility so that
//! reproducibility records never embed ANSI codes per §10.1.)
//!
//! ## See also
//!
//! - `harness/gates.rs` — the canonical gate registry consulted for
//!   metric definitions in [`render_drilldown`].
//! - `harness/repro.rs` — the on-disk record format consumed by
//!   [`render_diff`].

use crate::harness::gates::{
    registry, Comparator, GateResult, GateStatus, Priority,
};
use crate::harness::{Driver, ReleaseDecision, RunReport};
use std::fmt::Write as _;

// ---------------------------------------------------------------------------
// ANSI color helpers (used only when `use_color = true`).
// ---------------------------------------------------------------------------

const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const BRIGHT_RED: &str = "\x1b[91m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";

fn paint(buf: &mut String, color: &str, text: &str, use_color: bool) {
    if use_color {
        buf.push_str(color);
        buf.push_str(text);
        buf.push_str(RESET);
    } else {
        buf.push_str(text);
    }
}

fn status_label(status: GateStatus, use_color: bool) -> String {
    let mut s = String::new();
    match status {
        GateStatus::Pass => paint(&mut s, GREEN, "[PASS] ", use_color),
        GateStatus::Fail => paint(&mut s, RED, "[FAIL] ", use_color),
        GateStatus::Error => paint(&mut s, BRIGHT_RED, "[ERROR]", use_color),
    }
    s
}

fn driver_name(driver: Driver) -> &'static str {
    match driver {
        Driver::Locomo => "locomo",
        Driver::Longmemeval => "longmemeval",
        Driver::Cost => "cost",
        Driver::TestPreservation => "test-preservation",
        Driver::CognitiveRegression => "cognitive-regression",
        Driver::MigrationIntegrity => "migration-integrity",
    }
}

fn priority_label(priority: Priority) -> &'static str {
    match priority {
        Priority::P0Hard => "P0 hard",
        Priority::P0Expectation => "P0 expectation",
        Priority::P1Advisory => "P1 advisory",
    }
}

fn comparator_glyph(comp: Comparator) -> &'static str {
    match comp {
        Comparator::GreaterOrEqual => "≥",
        Comparator::LessOrEqual => "≤",
        Comparator::Equal => "=",
        Comparator::Compound => "∘",
    }
}

// ---------------------------------------------------------------------------
// §10.1 — Summary table
// ---------------------------------------------------------------------------

/// Header section of the summary table — design §10.1.
///
/// Captures build identity, run timing, and decision verdict. Inputs
/// are owned by the caller (rather than re-derived here) to keep this
/// module pure: timestamps come from the run record, build tags come
/// from `cargo` env, etc.
#[derive(Debug, Clone)]
pub struct SummaryHeader {
    /// e.g. `"engram @ a1b2c3d (v0.3.0-rc.1)"`.
    pub build_label: String,
    /// e.g. `"rustc 1.83.0 aarch64-apple-darwin"`.
    pub toolchain_label: String,
    /// ISO-8601 start timestamp.
    pub started_at: String,
    /// ISO-8601 finish timestamp.
    pub finished_at: String,
    /// Human-readable duration, e.g. `"5h42m"`.
    pub duration: String,
    /// Path to the run-artifact directory.
    pub artifact_path: String,
}

/// Render the design §10.1 summary table for a release-gate run.
///
/// Output is always plain ASCII (with optional ANSI color via
/// `use_color`); no Unicode box-drawing characters, no embedded
/// terminal control sequences other than color when requested.
///
/// Per design §10.1: `[PASS]` green, `[FAIL]` red, `[ERROR]` bright
/// red, only when `use_color = true`. CI logs (where `use_color` is
/// false) get plain text.
pub fn render_summary_table(
    header: &SummaryHeader,
    reports: &[RunReport],
    decision: &ReleaseDecision,
    use_color: bool,
) -> String {
    let mut out = String::new();

    // ── Header ──────────────────────────────────────────────────
    let _ = writeln!(out, "Engram v0.3 Release Gate Summary");
    let _ = writeln!(out, "================================");
    let _ = writeln!(out, "Build:    {}   {}", header.build_label, header.toolchain_label);
    let _ = writeln!(
        out,
        "Started:  {}   Finished: {}   Duration: {}",
        header.started_at, header.finished_at, header.duration
    );
    let _ = writeln!(out);

    // ── Group gates by priority across all driver reports ───────
    let mut p0_hard: Vec<(Driver, &GateResult)> = Vec::new();
    let mut p0_expectation: Vec<(Driver, &GateResult)> = Vec::new();
    let mut p1_advisory: Vec<(Driver, &GateResult)> = Vec::new();

    for report in reports {
        for gate in &report.gates {
            match gate.priority {
                Priority::P0Hard => p0_hard.push((report.driver, gate)),
                Priority::P0Expectation => p0_expectation.push((report.driver, gate)),
                Priority::P1Advisory => p1_advisory.push((report.driver, gate)),
            }
        }
    }

    if !p0_hard.is_empty() {
        let _ = writeln!(out, "P0 Ship Gates");
        let _ = writeln!(out, "─────────────");
        for (drv, gate) in &p0_hard {
            render_gate_line(&mut out, *drv, gate, use_color);
        }
        let _ = writeln!(out);
    }

    if !p0_expectation.is_empty() {
        let _ = writeln!(out, "P0 Expectation Gates");
        let _ = writeln!(out, "────────────────────");
        for (drv, gate) in &p0_expectation {
            render_gate_line(&mut out, *drv, gate, use_color);
        }
        let _ = writeln!(out);
    }

    if !p1_advisory.is_empty() {
        let _ = writeln!(out, "P1 Quality Gates");
        let _ = writeln!(out, "────────────────");
        for (drv, gate) in &p1_advisory {
            render_gate_line(&mut out, *drv, gate, use_color);
        }
        let _ = writeln!(out);
    }

    // ── Decision line ──────────────────────────────────────────
    match decision {
        ReleaseDecision::Ship => {
            let mut line = String::from("Decision: ");
            paint(&mut line, GREEN, "SHIP", use_color);
            line.push_str("  (all gates pass)");
            let _ = writeln!(out, "{}", line);
        }
        ReleaseDecision::ConditionalShip {
            overridden,
            p1_rationales,
        } => {
            let mut line = String::from("Decision: ");
            paint(&mut line, YELLOW, "CONDITIONAL SHIP", use_color);
            let _ = write!(
                line,
                "  ({} override(s), {} P1 rationale(s))",
                overridden.len(),
                p1_rationales.len()
            );
            let _ = writeln!(out, "{}", line);
        }
        ReleaseDecision::Block { failed_p0 } => {
            let mut line = String::from("Decision: ");
            paint(&mut line, RED, "BLOCK", use_color);
            let _ = write!(line, "  ({} blocking gate(s): {})", failed_p0.len(), failed_p0.join(", "));
            let _ = writeln!(out, "{}", line);
        }
    }

    let _ = writeln!(out, "Artifacts: {}", header.artifact_path);

    out
}

fn render_gate_line(out: &mut String, drv: Driver, gate: &GateResult, use_color: bool) {
    let label = status_label(gate.status, use_color);
    let observed = gate
        .observed
        .map(|v| format!("{:>7.3}", v))
        .unwrap_or_else(|| "    n/a".to_string());
    let threshold = gate
        .threshold
        .map(|v| format!("{:.3}", v))
        .unwrap_or_else(|| "n/a".to_string());
    let glyph = comparator_glyph(gate.comparator);
    let _ = writeln!(
        out,
        "  {} {:<10}  {:<28}  {} {} {}    [{}]",
        label,
        gate.goal,
        truncate(&gate.message, 32),
        observed,
        glyph,
        threshold,
        driver_name(drv),
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

// ---------------------------------------------------------------------------
// §10.2 — Per-gate drill-down
// ---------------------------------------------------------------------------

/// Per-gate drill-down — design §10.2.
///
/// Looks up the gate definition in the registry for the canonical
/// description, then prints: metric key, threshold + comparator,
/// observed measurement, delta vs the previous run if `prior` is
/// `Some`. Top-N contributing-episode rendering is owned by drivers
/// (each suite has its own contribution semantics) — the caller
/// passes the contribution lines via `contributions`.
///
/// Returns `None` if `goal` is not in the canonical registry —
/// callers should treat this as a CLI usage error (`engram-bench
/// explain GOAL-99.99` for a non-existent goal).
pub fn render_drilldown(
    goal: &str,
    current: &GateResult,
    prior: Option<&GateResult>,
    contributions: &[String],
    use_color: bool,
) -> Option<String> {
    let def = registry().iter().find(|d| d.goal == goal)?;
    let mut out = String::new();

    let _ = writeln!(out, "Gate {}  —  {}", def.goal, def.description);
    let _ = writeln!(out, "─────────────────────────────────────────────────");
    let _ = writeln!(out, "Priority:    {}", priority_label(def.priority));
    let _ = writeln!(out, "Metric:      {}", def.metric_key);
    let _ = writeln!(
        out,
        "Comparator:  {} ({})",
        comparator_glyph(def.comparator),
        comparator_desc(def.comparator)
    );

    let threshold_str = current
        .threshold
        .map(|v| format!("{:.4}", v))
        .unwrap_or_else(|| "n/a (compound)".to_string());
    let _ = writeln!(out, "Threshold:   {}", threshold_str);

    let observed_str = current
        .observed
        .map(|v| format!("{:.4}", v))
        .unwrap_or_else(|| "missing".to_string());
    let _ = writeln!(out, "Observed:    {}", observed_str);

    let mut status_line = String::from("Status:      ");
    status_line.push_str(&status_label(current.status, use_color));
    let _ = writeln!(out, "{}", status_line);
    let _ = writeln!(out, "Detail:      {}", current.message);

    if let Some(prev) = prior {
        let _ = writeln!(out);
        let _ = writeln!(out, "Δ vs prior run:");
        match (current.observed, prev.observed) {
            (Some(now), Some(then)) => {
                let delta = now - then;
                let arrow = if delta > 0.0 { "↑" } else if delta < 0.0 { "↓" } else { "=" };
                let _ = writeln!(
                    out,
                    "  prior={:.4}  current={:.4}  {} {:+.4}",
                    then, now, arrow, delta
                );
            }
            _ => {
                let _ = writeln!(out, "  prior or current measurement missing — no delta");
            }
        }
        if prev.status != current.status {
            let mut line = String::from("  Status changed: ");
            line.push_str(&status_label(prev.status, use_color));
            line.push_str(" → ");
            line.push_str(&status_label(current.status, use_color));
            let _ = writeln!(out, "{}", line);
        }
    }

    if !contributions.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "Top contributors:");
        for line in contributions {
            let _ = writeln!(out, "  {}", line);
        }
    }

    Some(out)
}

fn comparator_desc(c: Comparator) -> &'static str {
    match c {
        Comparator::GreaterOrEqual => "observed ≥ threshold",
        Comparator::LessOrEqual => "observed ≤ threshold",
        Comparator::Equal => "observed == threshold",
        Comparator::Compound => "compound (delegates to metric)",
    }
}

// ---------------------------------------------------------------------------
// §10.3 — Regression diff
// ---------------------------------------------------------------------------

/// One row in the regression-diff table — design §10.3.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffRow {
    /// GOAL identifier.
    pub goal: String,
    /// Status in the prior run.
    pub prior_status: GateStatus,
    /// Status in the current run.
    pub current_status: GateStatus,
    /// Observed value in the prior run, if present.
    pub prior_observed: Option<f64>,
    /// Observed value in the current run, if present.
    pub current_observed: Option<f64>,
}

impl DiffRow {
    /// Whether this row reflects a meaningful change worth surfacing.
    pub fn changed(&self) -> bool {
        self.prior_status != self.current_status || self.observed_delta().map(|d| d.abs() > 1e-9).unwrap_or(false)
    }

    /// `current - prior`, when both are present.
    pub fn observed_delta(&self) -> Option<f64> {
        match (self.current_observed, self.prior_observed) {
            (Some(c), Some(p)) => Some(c - p),
            _ => None,
        }
    }
}

/// Compute a per-gate diff between a prior run and the current run.
///
/// Pairs gates by `goal`. Gates present in only one side appear with
/// the missing side's status set to [`GateStatus::Error`] and observed
/// value `None` — surfacing "we lost a gate" as loudly as a status
/// flip per §4.4 (missing ≠ pass).
pub fn diff_runs(prior: &[GateResult], current: &[GateResult]) -> Vec<DiffRow> {
    use std::collections::BTreeMap;

    let mut prior_map: BTreeMap<&str, &GateResult> = BTreeMap::new();
    for g in prior {
        prior_map.insert(g.goal.as_str(), g);
    }
    let mut current_map: BTreeMap<&str, &GateResult> = BTreeMap::new();
    for g in current {
        current_map.insert(g.goal.as_str(), g);
    }

    let mut goals: Vec<&str> = prior_map.keys().chain(current_map.keys()).copied().collect();
    goals.sort();
    goals.dedup();

    goals
        .into_iter()
        .map(|goal| {
            let prior_g = prior_map.get(goal);
            let current_g = current_map.get(goal);
            DiffRow {
                goal: goal.to_string(),
                prior_status: prior_g.map(|g| g.status).unwrap_or(GateStatus::Error),
                current_status: current_g.map(|g| g.status).unwrap_or(GateStatus::Error),
                prior_observed: prior_g.and_then(|g| g.observed),
                current_observed: current_g.and_then(|g| g.observed),
            }
        })
        .collect()
}

/// Render the §10.3 regression-diff payload as plain text.
///
/// Only emits rows where the gate verdict changed OR the observed
/// metric drifted measurably (via [`DiffRow::changed`]). Stable runs
/// produce a one-line "no regressions detected" banner — keeps CI
/// channels quiet during healthy operation.
pub fn render_diff(diff: &[DiffRow], use_color: bool) -> String {
    let mut out = String::new();

    let changed: Vec<&DiffRow> = diff.iter().filter(|r| r.changed()).collect();
    if changed.is_empty() {
        let _ = writeln!(out, "No regressions detected ({} gates compared).", diff.len());
        return out;
    }

    let _ = writeln!(out, "Regression diff ({} gate(s) changed)", changed.len());
    let _ = writeln!(out, "─────────────────────────────────────");
    for row in changed {
        let prior = status_label(row.prior_status, use_color);
        let current = status_label(row.current_status, use_color);
        let delta = match row.observed_delta() {
            Some(d) => {
                let arrow = if d > 0.0 { "↑" } else if d < 0.0 { "↓" } else { "=" };
                format!("{} {:+.4}", arrow, d)
            }
            None => {
                let mut s = String::new();
                paint(&mut s, DIM, "(missing)", use_color);
                s
            }
        };
        let _ = writeln!(
            out,
            "  {:<10}  {} → {}  {}",
            row.goal, prior, current, delta
        );
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::gates::{Comparator, GateResult, GateStatus, Priority};
    use crate::harness::{Driver, ReleaseDecision, RunReport};
    use std::path::PathBuf;

    fn header() -> SummaryHeader {
        SummaryHeader {
            build_label: "engram @ deadbeef (v0.3.0-rc.1)".to_string(),
            toolchain_label: "rustc 1.83 aarch64-apple-darwin".to_string(),
            started_at: "2026-04-27T15:00:00Z".to_string(),
            finished_at: "2026-04-27T15:05:00Z".to_string(),
            duration: "5m".to_string(),
            artifact_path: "benchmarks/runs/test/".to_string(),
        }
    }

    fn pass_gate(goal: &str, metric: &str, observed: f64, threshold: f64) -> GateResult {
        GateResult {
            goal: goal.to_string(),
            metric_key: metric.to_string(),
            priority: Priority::P0Hard,
            comparator: Comparator::GreaterOrEqual,
            threshold: Some(threshold),
            observed: Some(observed),
            status: GateStatus::Pass,
            message: format!("{} ≥ {}", observed, threshold),
        }
    }

    fn fail_gate(goal: &str, metric: &str, observed: f64, threshold: f64) -> GateResult {
        GateResult {
            goal: goal.to_string(),
            metric_key: metric.to_string(),
            priority: Priority::P0Hard,
            comparator: Comparator::GreaterOrEqual,
            threshold: Some(threshold),
            observed: Some(observed),
            status: GateStatus::Fail,
            message: format!("{} < {}", observed, threshold),
        }
    }

    fn report(driver: Driver, gates: Vec<GateResult>) -> RunReport {
        RunReport {
            driver,
            record_path: PathBuf::from("/tmp/repro.toml"),
            gates,
            summary_json: serde_json::json!({}),
        }
    }

    #[test]
    fn summary_table_ship_decision_no_color_is_plain_ascii() {
        let reports = vec![report(
            Driver::Locomo,
            vec![pass_gate("GOAL-5.1", "locomo.overall", 0.712, 0.685)],
        )];
        let out = render_summary_table(&header(), &reports, &ReleaseDecision::Ship, false);
        assert!(out.contains("Engram v0.3 Release Gate Summary"));
        assert!(out.contains("[PASS]"));
        assert!(out.contains("GOAL-5.1"));
        assert!(out.contains("SHIP"));
        // No ANSI when use_color=false (parity with §10.1 record-clean rule).
        assert!(!out.contains("\x1b["));
    }

    #[test]
    fn summary_table_block_lists_failed_gates() {
        let reports = vec![report(
            Driver::Cost,
            vec![fail_gate("GOAL-5.4", "cost.per_episode", 3.21, 3.0)],
        )];
        let decision = ReleaseDecision::Block {
            failed_p0: vec!["GOAL-5.4".to_string()],
        };
        let out = render_summary_table(&header(), &reports, &decision, false);
        assert!(out.contains("[FAIL]"));
        assert!(out.contains("BLOCK"));
        assert!(out.contains("GOAL-5.4"));
    }

    #[test]
    fn summary_table_color_emits_ansi_when_enabled() {
        let reports = vec![report(
            Driver::Locomo,
            vec![pass_gate("GOAL-5.1", "locomo.overall", 0.712, 0.685)],
        )];
        let out = render_summary_table(&header(), &reports, &ReleaseDecision::Ship, true);
        assert!(out.contains("\x1b[32m"), "expected green ANSI escape");
        assert!(out.contains("\x1b[0m"), "expected ANSI reset");
    }

    #[test]
    fn drilldown_includes_metric_definition_and_delta() {
        let current = pass_gate("GOAL-5.1", "locomo.overall", 0.712, 0.685);
        let prior = pass_gate("GOAL-5.1", "locomo.overall", 0.700, 0.685);
        let out = render_drilldown(
            "GOAL-5.1",
            &current,
            Some(&prior),
            &["episode-7: 0.012 contribution".to_string()],
            false,
        )
        .expect("GOAL-5.1 must be in the registry");
        assert!(out.contains("LOCOMO"));
        assert!(out.contains("Comparator:"));
        assert!(out.contains("0.7120"));
        assert!(out.contains("Δ vs prior run"));
        assert!(out.contains("+0.0120"));
        assert!(out.contains("episode-7"));
    }

    #[test]
    fn drilldown_unknown_goal_returns_none() {
        let g = pass_gate("GOAL-999.99", "nonexistent.metric", 1.0, 0.0);
        let out = render_drilldown("GOAL-999.99", &g, None, &[], false);
        assert!(out.is_none());
    }

    #[test]
    fn diff_detects_status_flip() {
        let prior = vec![pass_gate("GOAL-5.1", "locomo.overall", 0.700, 0.685)];
        let current = vec![fail_gate("GOAL-5.1", "locomo.overall", 0.650, 0.685)];
        let diff = diff_runs(&prior, &current);
        assert_eq!(diff.len(), 1);
        let row = &diff[0];
        assert!(row.changed());
        assert_eq!(row.prior_status, GateStatus::Pass);
        assert_eq!(row.current_status, GateStatus::Fail);
        assert!((row.observed_delta().unwrap() - (-0.05)).abs() < 1e-9);
    }

    #[test]
    fn diff_missing_gate_surfaces_as_error() {
        // Lost a gate between runs — surfaces as Error per §4.4.
        let prior = vec![pass_gate("GOAL-5.1", "locomo.overall", 0.700, 0.685)];
        let current: Vec<GateResult> = vec![];
        let diff = diff_runs(&prior, &current);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].current_status, GateStatus::Error);
    }

    #[test]
    fn diff_no_changes_emits_quiet_banner() {
        let g = pass_gate("GOAL-5.1", "locomo.overall", 0.700, 0.685);
        let diff = diff_runs(&[g.clone()], &[g]);
        let out = render_diff(&diff, false);
        assert!(out.contains("No regressions detected"));
    }

    #[test]
    fn diff_render_lists_changed_only() {
        let prior = vec![
            pass_gate("GOAL-5.1", "locomo.overall", 0.700, 0.685),
            pass_gate("GOAL-5.4", "cost.per_episode", 2.5, 3.0),
        ];
        let current = vec![
            pass_gate("GOAL-5.1", "locomo.overall", 0.700, 0.685), // unchanged
            fail_gate("GOAL-5.4", "cost.per_episode", 3.21, 3.0),  // flipped
        ];
        let diff = diff_runs(&prior, &current);
        let out = render_diff(&diff, false);
        assert!(out.contains("GOAL-5.4"));
        assert!(!out.contains("GOAL-5.1"), "unchanged gate should not appear: {}", out);
    }
}
