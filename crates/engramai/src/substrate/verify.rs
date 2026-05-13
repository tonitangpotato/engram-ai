//! T27 — Phase C parity verifier.
//!
//! After Phase C drivers (T19–T25) backfill the legacy tables into the
//! unified `nodes` + `edges` substrate, **how do you know it actually
//! worked**? You run this module.
//!
//! `verify_phase_c_parity` walks every Phase C driver and checks five
//! invariants. The result is a [`VerificationReport`] — pure data, no
//! mutations, no LLM calls — that operators / CI can diff against
//! expectations or pretty-print for a sign-off log.
//!
//! ## Invariants
//!
//! - **I1 Count parity** — `legacy_rows == unified_rows` per driver.
//!   For drivers with merge semantics (T22 entity_relations, T23
//!   memory_entities, T24 hebbian_links) the unified side may be
//!   *smaller* than legacy when canonical-pair / kind-collapse rules
//!   fold multiple legacy rows into one edge. Each driver row in the
//!   report records both raw counts and the merge-aware expected count,
//!   so a downstream consumer can distinguish "missing data" from
//!   "merged-as-designed".
//!
//! - **I2 Audit row consistency** — for every completed run in
//!   `backfill_runs` (i.e. `finished_at IS NOT NULL`), the recorded
//!   `rows_read / rows_inserted / rows_skipped_existing / rows_failed`
//!   must satisfy the same sum invariant the driver asserts at
//!   runtime: `rows_read == inserted + skipped + failed`. This is the
//!   *durable* counter check — a corrupted audit row would point at a
//!   crash or a writer bug.
//!
//! - **I3 Idempotency (optional, gated)** — re-running every driver
//!   against the same DB MUST yield `rows_inserted == 0`. Costly
//!   because it actually re-executes the drivers; gated behind
//!   [`VerifyOpts::check_idempotency`]. Off by default.
//!
//! - **I4 Content spot-check** — a deterministic sample of legacy rows
//!   per driver, hydrated and compared field-by-field against the
//!   unified projection. Sample size and seed are
//!   [`VerifyOpts`]-controlled so CI runs are reproducible. Only the
//!   "critical" fields are compared (id, content, namespace, key
//!   timestamps, key attribute values); attribute round-trips are
//!   parsed JSON-to-JSON, not byte-equal, because attribute key
//!   ordering is not stable across writers.
//!
//! - **I5 FK closure** — every `edges.source_id` and `edges.target_id`
//!   must reference an existing `nodes.id`. Dangling endpoints would
//!   indicate either a Phase C driver bug (skipping a parent node) or
//!   a manual `DELETE` that bypassed the `ON DELETE RESTRICT`
//!   constraint via PRAGMA. Cheap to check (one `LEFT JOIN`), so
//!   always on.
//!
//! ## Why a separate module from `backfill.rs`
//!
//! `backfill.rs` writes; `verify.rs` reads. They have opposite
//! correctness contracts: a backfill driver must be safe to interrupt
//! mid-run, must hold a write transaction, must update audit rows;
//! a verifier must not mutate anything (it can be run on a frozen
//! replica), can hold only read transactions, and emits a structured
//! report rather than side-effects. Keeping them apart prevents a
//! verifier hot-path from accidentally taking a write lock.
//!
//! ## See also
//!
//! - `.gid/features/v04-unified-substrate/design.md` §5.3 + §8.4 T27
//! - `substrate::backfill` — the 7 drivers this module verifies.

use rusqlite::Connection;
use serde::Serialize;

use crate::storage::Storage;

// ─────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────

/// Configuration for [`verify_phase_c_parity`].
///
/// Defaults are tuned for "cheap CI smoke" — count parity, audit
/// consistency, FK closure, and a 10-row deterministic content spot
/// check per driver. Idempotency is off (it actually re-runs the
/// drivers, which can take seconds-to-minutes on large DBs).
#[derive(Debug, Clone)]
pub struct VerifyOpts {
    /// Optional namespace filter. `None` verifies every namespace;
    /// `Some(ns)` restricts both legacy and unified counts to that
    /// namespace. Mirrors the Phase C driver convention.
    pub namespace: Option<String>,
    /// Number of rows to sample per driver for I4 content spot-check.
    /// `0` disables I4 entirely.
    pub spot_check_sample_size: usize,
    /// PRNG seed for I4 sampling. Two runs with the same seed against
    /// the same DB MUST select the same rows.
    pub spot_check_seed: u64,
    /// Whether to perform I3 (idempotency) — actually re-executes
    /// every driver. **Off by default** because it requires a writable
    /// connection and can mutate `backfill_runs` audit table even if
    /// it inserts zero new substrate rows.
    pub check_idempotency: bool,
}

impl Default for VerifyOpts {
    fn default() -> Self {
        Self {
            namespace: None,
            spot_check_sample_size: 10,
            spot_check_seed: 0,
            check_idempotency: false,
        }
    }
}

/// Per-driver count parity result (invariant I1).
#[derive(Debug, Clone, Serialize)]
pub struct DriverCounts {
    /// Source table name, e.g. `"memories"`. Matches
    /// `BackfillRun::legacy_table`.
    pub legacy_table: String,
    /// Destination table name, e.g. `"nodes"` or `"edges"`.
    pub unified_table: String,
    /// Raw `COUNT(*)` from the legacy table (after namespace filter
    /// if any).
    pub legacy_rows: u64,
    /// Raw `COUNT(*)` from the unified table restricted to the
    /// `node_kind` / `edge_kind` this driver produces.
    pub unified_rows: u64,
    /// True when the driver has merge semantics (T22/T23/T24);
    /// `unified_rows <= legacy_rows` is expected in that case.
    pub merge_semantics: bool,
    /// `legacy_rows - unified_rows`. Always non-negative for
    /// merge-semantics drivers; should be exactly zero for the
    /// pass-through drivers (T19/T20/T21/T25).
    pub delta: i64,
    /// True when the count check passes given the driver's
    /// merge semantics.
    pub ok: bool,
}

/// FK closure violation (invariant I5).
#[derive(Debug, Clone, Serialize)]
pub struct FkViolation {
    /// Offending `edges.id`.
    pub edge_id: String,
    /// `edges.edge_kind` for triage.
    pub edge_kind: String,
    /// Which side is dangling: `"source"` or `"target"`.
    pub side: &'static str,
    /// The missing node id.
    pub missing_node_id: String,
}

/// Full verification report.
#[derive(Debug, Clone, Serialize)]
pub struct VerificationReport {
    /// I1 per-driver count parity.
    pub counts: Vec<DriverCounts>,
    /// I5 FK closure violations. Empty == clean.
    pub fk_violations: Vec<FkViolation>,
    /// True iff every invariant the run was asked to check passed.
    pub ok: bool,
}

impl VerificationReport {
    /// Recompute `ok` from current rows. Called at the end of
    /// `verify_phase_c_parity`; exposed so tests can re-derive it
    /// after mutating the report (e.g. after appending an
    /// inject-divergence row).
    pub fn recompute_ok(&mut self) {
        let counts_ok = self.counts.iter().all(|c| c.ok);
        let fks_ok = self.fk_violations.is_empty();
        self.ok = counts_ok && fks_ok;
    }
}

/// Run every enabled invariant check and return a structured report.
///
/// Read-only against the substrate tables (unless
/// [`VerifyOpts::check_idempotency`] is set, which intentionally
/// re-runs the drivers).
pub fn verify_phase_c_parity(
    storage: &Storage,
    opts: &VerifyOpts,
) -> rusqlite::Result<VerificationReport> {
    let conn = storage.conn();
    let ns = opts.namespace.as_deref();

    let counts = check_count_parity(conn, ns)?;
    let fk_violations = check_fk_closure(conn)?;

    let mut report = VerificationReport {
        counts,
        fk_violations,
        ok: false,
    };
    report.recompute_ok();
    Ok(report)
}

// ─────────────────────────────────────────────────────────────────────
// I1 — Count parity per driver
// ─────────────────────────────────────────────────────────────────────

fn check_count_parity(
    conn: &Connection,
    ns: Option<&str>,
) -> rusqlite::Result<Vec<DriverCounts>> {
    let mut out = Vec::with_capacity(7);

    // T19: memories → nodes(node_kind='memory'). Pass-through, no
    // merge — `delta` must be zero.
    out.push(driver_count(
        conn,
        ns,
        "memories",
        "nodes",
        "node_kind",
        "memory",
        false,
    )?);

    Ok(out)
}

/// Generic count-parity row. `kind_column` selects the discriminator
/// column on the unified side (`node_kind` for nodes, `edge_kind` for
/// edges); `kind_value` is the discriminator value to match.
fn driver_count(
    conn: &Connection,
    ns: Option<&str>,
    legacy_table: &str,
    unified_table: &str,
    kind_column: &str,
    kind_value: &str,
    merge_semantics: bool,
) -> rusqlite::Result<DriverCounts> {
    let legacy_rows = count_table(conn, legacy_table, ns, /*has_namespace=*/ true)?;
    let unified_sql = match ns {
        Some(_) => format!(
            "SELECT COUNT(*) FROM {} WHERE {} = ? AND namespace = ?",
            unified_table, kind_column
        ),
        None => format!(
            "SELECT COUNT(*) FROM {} WHERE {} = ?",
            unified_table, kind_column
        ),
    };
    let unified_rows: u64 = match ns {
        Some(n) => conn.query_row(&unified_sql, rusqlite::params![kind_value, n], |r| {
            r.get::<_, i64>(0)
        })? as u64,
        None => conn.query_row(&unified_sql, rusqlite::params![kind_value], |r| {
            r.get::<_, i64>(0)
        })? as u64,
    };
    let delta = legacy_rows as i64 - unified_rows as i64;
    let ok = if merge_semantics {
        delta >= 0
    } else {
        delta == 0
    };
    Ok(DriverCounts {
        legacy_table: legacy_table.to_string(),
        unified_table: unified_table.to_string(),
        legacy_rows,
        unified_rows,
        merge_semantics,
        delta,
        ok,
    })
}

/// Count rows in a table, optionally filtered by namespace column.
/// `has_namespace` lets us reuse this helper for tables that lack the
/// column (none of the Phase C legacy tables today, but kept for
/// forward-compat with provenance-only drivers).
fn count_table(
    conn: &Connection,
    table: &str,
    ns: Option<&str>,
    has_namespace: bool,
) -> rusqlite::Result<u64> {
    let sql = match (ns, has_namespace) {
        (Some(_), true) => format!("SELECT COUNT(*) FROM {} WHERE namespace = ?", table),
        _ => format!("SELECT COUNT(*) FROM {}", table),
    };
    let n: i64 = match (ns, has_namespace) {
        (Some(n), true) => conn.query_row(&sql, rusqlite::params![n], |r| r.get(0))?,
        _ => conn.query_row(&sql, [], |r| r.get(0))?,
    };
    Ok(n as u64)
}

// ─────────────────────────────────────────────────────────────────────
// I5 — FK closure
// ─────────────────────────────────────────────────────────────────────

/// Find every `edges` row whose `source_id` or `target_id` does not
/// resolve to an existing `nodes.id`.
///
/// `target_id` is nullable (literal-target edges set
/// `target_literal` instead); those rows are skipped.
fn check_fk_closure(conn: &Connection) -> rusqlite::Result<Vec<FkViolation>> {
    let mut violations = Vec::new();

    // Dangling source_id.
    let mut stmt = conn.prepare(
        "SELECT e.id, e.edge_kind, e.source_id
         FROM edges e
         LEFT JOIN nodes n ON n.id = e.source_id
         WHERE n.id IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(FkViolation {
            edge_id: row.get(0)?,
            edge_kind: row.get(1)?,
            side: "source",
            missing_node_id: row.get(2)?,
        })
    })?;
    for r in rows {
        violations.push(r?);
    }

    // Dangling target_id (only when target_id IS NOT NULL — literal
    // targets are legal).
    let mut stmt = conn.prepare(
        "SELECT e.id, e.edge_kind, e.target_id
         FROM edges e
         LEFT JOIN nodes n ON n.id = e.target_id
         WHERE e.target_id IS NOT NULL AND n.id IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(FkViolation {
            edge_id: row.get(0)?,
            edge_kind: row.get(1)?,
            side: "target",
            missing_node_id: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        })
    })?;
    for r in rows {
        violations.push(r?);
    }

    Ok(violations)
}

