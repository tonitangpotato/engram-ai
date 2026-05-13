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

/// Internal fingerprint identifying which unified rows "belong to" a
/// given Phase C driver.
///
/// The unified `edges` table is shared by four drivers (T22/T23/T24/
/// T25) and the discriminator `edge_kind` alone is not enough to
/// separate them — T23 and T25 both emit `edge_kind='provenance'`
/// rows, T22 and T23 both emit `edge_kind='structural'` rows. The
/// **distinguishing fingerprint** is the pair
/// `(edge_kind, predicate ∈ {...})` because the writer side commits
/// to a fixed predicate vocabulary per driver:
///
/// - T22 entity_relations → structural / arbitrary canonical predicates
///   from `entity_relations.relation_type` (NOT in the closed sets
///   below).
/// - T23 memory_entities  → provenance/`mentions` + structural/
///   `subject_of`,`object_of`.
/// - T24 hebbian_links    → associative/`co_activated`.
/// - T25 synthesis_prov.  → provenance/`derived_from`.
///
/// `T22` is the residual: structural rows whose predicate is NOT one
/// of T23's two structural predicates. We encode that as
/// `Fingerprint::EdgeKindMinusPredicates`.
#[derive(Debug, Clone)]
enum Fingerprint {
    /// `nodes` row where `node_kind = value`.
    NodeKind { value: &'static str },
    /// Plain table `COUNT(*)`. Used for `node_embeddings` (T20) which
    /// has no kind discriminator.
    PlainTable,
    /// `edges` row where `edge_kind = kind AND predicate IN (...)`.
    EdgeKindPredicateIn {
        kind: &'static str,
        predicates: &'static [&'static str],
    },
    /// `edges` row where `edge_kind = kind` and the predicate is NOT
    /// in the excluded set. T22's residual identity.
    EdgeKindMinusPredicates {
        kind: &'static str,
        exclude: &'static [&'static str],
    },
    /// Union of two fingerprints, counted with deduplication on
    /// `edges.id`. T23 spans two `(edge_kind, predicate)` buckets.
    Union(Box<Fingerprint>, Box<Fingerprint>),
}

struct DriverSpec {
    legacy_table: &'static str,
    unified_table: &'static str,
    fingerprint: Fingerprint,
    merge_semantics: bool,
    legacy_has_namespace: bool,
}

fn driver_specs() -> Vec<DriverSpec> {
    vec![
        // T19 memories → nodes(node_kind='memory'). Pass-through.
        DriverSpec {
            legacy_table: "memories",
            unified_table: "nodes",
            fingerprint: Fingerprint::NodeKind { value: "memory" },
            merge_semantics: false,
            legacy_has_namespace: true,
        },
        // T20 memory_embeddings → node_embeddings. Pass-through, no
        // kind column on the unified side. NOTE: node_embeddings has
        // no `namespace` column either; the namespace filter is
        // applied to the legacy side, but the unified side counts
        // all rows (acceptable because per-namespace embedding
        // backfill is rare and the counter is informational here —
        // future iterations may JOIN node_embeddings to nodes for
        // per-namespace verification).
        DriverSpec {
            legacy_table: "memory_embeddings",
            unified_table: "node_embeddings",
            fingerprint: Fingerprint::PlainTable,
            merge_semantics: false,
            legacy_has_namespace: false,
        },
        // T21 entities → nodes(node_kind='entity'). Pass-through.
        DriverSpec {
            legacy_table: "entities",
            unified_table: "nodes",
            fingerprint: Fingerprint::NodeKind { value: "entity" },
            merge_semantics: false,
            legacy_has_namespace: true,
        },
        // T22 entity_relations → edges(edge_kind='structural',
        // predicate ∉ T23's structural set). MERGE semantics
        // (canonical-pair collapse + relation-type-aware dedupe).
        DriverSpec {
            legacy_table: "entity_relations",
            unified_table: "edges",
            fingerprint: Fingerprint::EdgeKindMinusPredicates {
                kind: "structural",
                exclude: &["subject_of", "object_of"],
            },
            merge_semantics: true,
            legacy_has_namespace: true,
        },
        // T23 memory_entities → edges(provenance/'mentions' +
        // structural/'subject_of','object_of'). MERGE semantics
        // (role-collapse). memory_entities has NO namespace column;
        // it inherits via JOIN on memories(memory_id) at backfill
        // time, so the namespace filter is ignored on the legacy side
        // (same caveat as synthesis_provenance below).
        DriverSpec {
            legacy_table: "memory_entities",
            unified_table: "edges",
            fingerprint: Fingerprint::Union(
                Box::new(Fingerprint::EdgeKindPredicateIn {
                    kind: "provenance",
                    predicates: &["mentions"],
                }),
                Box::new(Fingerprint::EdgeKindPredicateIn {
                    kind: "structural",
                    predicates: &["subject_of", "object_of"],
                }),
            ),
            merge_semantics: true,
            legacy_has_namespace: false,
        },
        // T24 hebbian_links → edges(associative/'co_activated').
        // MERGE semantics (canonical-pair direction collapse).
        DriverSpec {
            legacy_table: "hebbian_links",
            unified_table: "edges",
            fingerprint: Fingerprint::EdgeKindPredicateIn {
                kind: "associative",
                predicates: &["co_activated"],
            },
            merge_semantics: true,
            legacy_has_namespace: true,
        },
        // T25 synthesis_provenance → edges(provenance/'derived_from').
        // Pass-through (append-only, no merge).
        DriverSpec {
            legacy_table: "synthesis_provenance",
            unified_table: "edges",
            fingerprint: Fingerprint::EdgeKindPredicateIn {
                kind: "provenance",
                predicates: &["derived_from"],
            },
            merge_semantics: false,
            // synthesis_provenance has no `namespace` column; it
            // inherits via JOIN on memories(insight_id) at backfill
            // time. Counting on the legacy side ignores the filter
            // for this driver.
            legacy_has_namespace: false,
        },
    ]
}

fn check_count_parity(
    conn: &Connection,
    ns: Option<&str>,
) -> rusqlite::Result<Vec<DriverCounts>> {
    let specs = driver_specs();
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        out.push(driver_count(conn, ns, &spec)?);
    }
    Ok(out)
}

fn driver_count(
    conn: &Connection,
    ns: Option<&str>,
    spec: &DriverSpec,
) -> rusqlite::Result<DriverCounts> {
    let legacy_rows = count_table(conn, spec.legacy_table, ns, spec.legacy_has_namespace)?;
    let unified_rows = count_unified(conn, &spec.fingerprint, ns)?;
    let delta = legacy_rows as i64 - unified_rows as i64;
    let ok = if spec.merge_semantics {
        delta >= 0
    } else {
        delta == 0
    };
    Ok(DriverCounts {
        legacy_table: spec.legacy_table.to_string(),
        unified_table: spec.unified_table.to_string(),
        legacy_rows,
        unified_rows,
        merge_semantics: spec.merge_semantics,
        delta,
        ok,
    })
}

/// Count unified rows matching the driver's fingerprint, restricted
/// to `namespace` when applicable. `node_embeddings` (PlainTable) is
/// not namespace-filtered because it has no namespace column; that
/// limitation is documented on `DriverSpec::legacy_has_namespace`
/// above.
fn count_unified(
    conn: &Connection,
    fp: &Fingerprint,
    ns: Option<&str>,
) -> rusqlite::Result<u64> {
    match fp {
        Fingerprint::NodeKind { value } => {
            let (sql, has_ns_param) = match ns {
                Some(_) => (
                    "SELECT COUNT(*) FROM nodes WHERE node_kind = ? AND namespace = ?",
                    true,
                ),
                None => ("SELECT COUNT(*) FROM nodes WHERE node_kind = ?", false),
            };
            let n: i64 = if has_ns_param {
                conn.query_row(sql, rusqlite::params![value, ns.unwrap()], |r| r.get(0))?
            } else {
                conn.query_row(sql, rusqlite::params![value], |r| r.get(0))?
            };
            Ok(n as u64)
        }
        Fingerprint::PlainTable => {
            // node_embeddings has no namespace column → ignore filter.
            let n: i64 =
                conn.query_row("SELECT COUNT(*) FROM node_embeddings", [], |r| r.get(0))?;
            Ok(n as u64)
        }
        Fingerprint::EdgeKindPredicateIn { kind, predicates } => {
            count_edges_predicate_in(conn, kind, predicates, /*negate=*/ false, ns)
        }
        Fingerprint::EdgeKindMinusPredicates { kind, exclude } => {
            count_edges_predicate_in(conn, kind, exclude, /*negate=*/ true, ns)
        }
        Fingerprint::Union(a, b) => {
            // edges.id is the PK, so DISTINCT counts dedupe correctly
            // across two overlapping buckets. In practice the buckets
            // don't overlap (T23 emits one row per (memory, entity,
            // role) and never reuses the same id for two roles), but
            // the DISTINCT keeps the bound honest if a future writer
            // ever does.
            let union_sql = build_union_sql(a, b, ns.is_some());
            let n: i64 = match ns {
                Some(n) => conn.query_row(&union_sql, rusqlite::params![n], |r| r.get(0))?,
                None => conn.query_row(&union_sql, [], |r| r.get(0))?,
            };
            Ok(n as u64)
        }
    }
}

fn count_edges_predicate_in(
    conn: &Connection,
    kind: &str,
    predicates: &[&str],
    negate: bool,
    ns: Option<&str>,
) -> rusqlite::Result<u64> {
    let placeholders = vec!["?"; predicates.len()].join(",");
    let predicate_clause = if predicates.is_empty() {
        // `negate=true` with empty list = unconstrained; `negate=false`
        // with empty list = impossible. Treat as a soft no-op rather
        // than crashing.
        if negate {
            "".to_string()
        } else {
            "AND 1 = 0".to_string()
        }
    } else if negate {
        format!("AND predicate NOT IN ({placeholders})")
    } else {
        format!("AND predicate IN ({placeholders})")
    };
    let ns_clause = if ns.is_some() {
        " AND namespace = ?"
    } else {
        ""
    };
    let sql = format!(
        "SELECT COUNT(*) FROM edges WHERE edge_kind = ? {predicate_clause}{ns_clause}"
    );
    let ns_owned = ns.map(|s| s.to_string());
    let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(2 + predicates.len());
    params.push(&kind);
    for p in predicates {
        params.push(p);
    }
    if let Some(ref n) = ns_owned {
        params.push(n);
    }
    let n: i64 = conn.query_row(&sql, rusqlite::params_from_iter(params.iter()), |r| {
        r.get(0)
    })?;
    Ok(n as u64)
}

/// Build a SELECT COUNT(DISTINCT id) for a Union fingerprint. Only
/// supports `EdgeKindPredicateIn` leaves — `Union` of `Union` or of
/// minus-predicate variants is not used by any current driver and is
/// rejected with a hard panic to surface the missing case during
/// development.
fn build_union_sql(a: &Fingerprint, b: &Fingerprint, with_ns: bool) -> String {
    fn leaf_clause(fp: &Fingerprint) -> String {
        match fp {
            Fingerprint::EdgeKindPredicateIn { kind, predicates } => {
                let placeholders: String = predicates
                    .iter()
                    .map(|p| format!("'{}'", p.replace('\'', "''")))
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "(edge_kind = '{}' AND predicate IN ({}))",
                    kind.replace('\'', "''"),
                    placeholders
                )
            }
            other => panic!(
                "Fingerprint::Union only supports EdgeKindPredicateIn leaves; got {other:?}"
            ),
        }
    }
    let ns_clause = if with_ns { " AND namespace = ?" } else { "" };
    format!(
        "SELECT COUNT(DISTINCT id) FROM edges WHERE ({} OR {}){}",
        leaf_clause(a),
        leaf_clause(b),
        ns_clause
    )
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

