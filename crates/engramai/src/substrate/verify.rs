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

use rusqlite::{Connection, OptionalExtension};
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
    /// True if the namespace filter was applied to the legacy-side
    /// count. False when the driver was asked to filter by namespace
    /// but the legacy table has no `namespace` column (memory_entities,
    /// memory_embeddings, synthesis_provenance — see
    /// `DriverSpec::legacy_has_namespace`). In that case `legacy_rows`
    /// is a GLOBAL count rather than a namespace-scoped one, and the
    /// `delta` is only meaningful when `opts.namespace` is `None`.
    ///
    /// Always `true` when `opts.namespace` is `None` (no filter
    /// requested means filter trivially "applied").
    pub legacy_ns_filter_applied: bool,
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

/// Audit row counter inconsistency (invariant I2).
///
/// Surfaced for every completed `backfill_runs` row where
/// `rows_read != rows_inserted + rows_skipped_existing + rows_failed`.
/// In a healthy DB this list is empty — the driver code asserts the
/// same invariant at runtime, so a violation here implies either
/// (a) a crash between sub-counter writes and the final commit,
/// (b) a future writer that bypasses the helper, or
/// (c) manual editing of the audit table.
#[derive(Debug, Clone, Serialize)]
pub struct AuditViolation {
    /// Offending `backfill_runs.run_id`.
    pub run_id: String,
    /// Source table the run targeted, for triage.
    pub legacy_table: String,
    pub rows_read: u64,
    pub rows_inserted: u64,
    pub rows_skipped_existing: u64,
    pub rows_failed: u64,
    /// `inserted + skipped + failed`. Should equal `rows_read`.
    pub computed_sum: u64,
}

/// Content spot-check mismatch (invariant I4).
///
/// Surfaced when a legacy row's projection into the unified table
/// disagrees on a critical field. "Critical" means: id, namespace,
/// layer / memory_type / kind discriminators, content, key
/// timestamps, and the attribute JSON parsed value-by-value. Counter
/// fields on merge-semantics drivers (weight, coactivation_count,
/// etc.) are intentionally excluded because they SUM across legacy
/// rows.
#[derive(Debug, Clone, Serialize)]
pub struct ContentMismatch {
    /// Source table the sample was drawn from.
    pub legacy_table: String,
    /// Offending row id (legacy and unified ids match by design
    /// for the drivers I4 currently covers; recorded once).
    pub row_id: String,
    /// Which field disagreed. Free-form so the message can name
    /// nested JSON paths like `"attributes.tag"`.
    pub field: String,
    /// Legacy side value, stringified.
    pub legacy: String,
    /// Unified side value, stringified.
    pub unified: String,
}

/// Idempotency violation (invariant I3).
///
/// Surfaced when a re-run of a Phase C driver against an
/// already-backfilled DB inserts more than zero new rows. The
/// driver's `BackfillRun::rows_inserted` is the canonical measure;
/// `rows_skipped_existing` should equal `rows_read` on a clean
/// re-run.
///
/// Gated behind [`VerifyOpts::check_idempotency`] because it
/// actually re-executes the drivers. Requires `&mut Storage` and
/// the dedicated [`verify_phase_c_parity_mut`] entry point.
#[derive(Debug, Clone, Serialize)]
pub struct IdempotencyViolation {
    /// Source table whose driver misbehaved.
    pub legacy_table: String,
    /// `rows_inserted` from the re-run. Should be 0.
    pub rows_inserted_on_rerun: u64,
    /// `rows_read` from the re-run. Reported for triage so an
    /// operator can see how big the run was.
    pub rows_read_on_rerun: u64,
}

/// Full verification report.
#[derive(Debug, Clone, Serialize)]
pub struct VerificationReport {
    /// I1 per-driver count parity.
    pub counts: Vec<DriverCounts>,
    /// I2 audit row inconsistencies. Empty == clean.
    pub audit_violations: Vec<AuditViolation>,
    /// I3 idempotency violations. Empty == clean. Always empty
    /// when invoked via [`verify_phase_c_parity`] (the read-only
    /// entry point) because I3 needs `&mut Storage`; populated only
    /// by [`verify_phase_c_parity_mut`] when
    /// [`VerifyOpts::check_idempotency`] is true.
    pub idempotency_violations: Vec<IdempotencyViolation>,
    /// I4 content spot-check mismatches. Empty == clean (or check
    /// disabled by `spot_check_sample_size == 0`).
    pub content_mismatches: Vec<ContentMismatch>,
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
        let audit_ok = self.audit_violations.is_empty();
        let content_ok = self.content_mismatches.is_empty();
        let idempotency_ok = self.idempotency_violations.is_empty();
        let fks_ok = self.fk_violations.is_empty();
        self.ok = counts_ok && audit_ok && content_ok && idempotency_ok && fks_ok;
    }
}

/// Run every read-only invariant check and return a structured
/// report. Read-only against substrate tables (no mutations).
///
/// **Does NOT run I3 (idempotency)** even when
/// [`VerifyOpts::check_idempotency`] is true — I3 needs `&mut
/// Storage` and is reachable only via
/// [`verify_phase_c_parity_mut`]. The flag is honored by the `_mut`
/// variant; here it's a no-op so a tooling caller can pass the
/// same `VerifyOpts` to either entry point.
pub fn verify_phase_c_parity(
    storage: &Storage,
    opts: &VerifyOpts,
) -> rusqlite::Result<VerificationReport> {
    let conn = storage.conn();
    let ns = opts.namespace.as_deref();

    let counts = check_count_parity(conn, ns)?;
    let audit_violations = check_audit_consistency(conn)?;
    let content_mismatches = check_content_spot_check(conn, ns, opts)?;
    let fk_violations = check_fk_closure(conn)?;

    let mut report = VerificationReport {
        counts,
        audit_violations,
        idempotency_violations: Vec::new(),
        content_mismatches,
        fk_violations,
        ok: false,
    };
    report.recompute_ok();
    Ok(report)
}

/// Same as [`verify_phase_c_parity`] but ALSO runs I3 (idempotency)
/// when [`VerifyOpts::check_idempotency`] is true. Takes `&mut
/// Storage` because the re-run requires it.
///
/// **Cost.** Re-executes every Phase C driver. On a freshly
/// backfilled DB the work is bounded by `rows_read` SELECTs +
/// zero INSERTs; on a stale DB it could insert real rows (which
/// would itself be a violation of I3 — that's the point). Each
/// re-run also appends a new audit row to `backfill_runs`. The
/// audit table is append-only by design; this is not a leak.
///
/// **Audit-row growth.** Every call with `check_idempotency=true`
/// appends 7 rows to `backfill_runs` (one per driver). Operators
/// running I3 in CI on every merge should expect linear growth in
/// this table over time. The verifier itself stays correct (I2 only
/// flags rows where `rows_read != sum`; idempotency re-runs trivially
/// satisfy `rows_read == 0 + rows_read + 0`), but the table size
/// climbs.
///
/// **I2 ordering.** The I2 audit consistency check runs BEFORE the
/// I3 re-run in this function, so the `audit_violations` field in
/// the returned report reflects state PRE-rerun. The 7 new audit
/// rows from I3 are visible only on the next call to this entry
/// point (or any call to the read-only variant). The new rows are
/// guaranteed I2-clean by construction, so this ordering does not
/// hide real violations.
///
/// When `check_idempotency = false` this entry point is exactly
/// equivalent to [`verify_phase_c_parity`].
pub fn verify_phase_c_parity_mut(
    storage: &mut Storage,
    opts: &VerifyOpts,
) -> rusqlite::Result<VerificationReport> {
    // Run all read-only invariants first against a `&` borrow.
    let mut report = verify_phase_c_parity(storage, opts)?;

    if opts.check_idempotency {
        let idempotency_violations = check_idempotency(storage, opts.namespace.as_deref())?;
        report.idempotency_violations = idempotency_violations;
        report.recompute_ok();
    }
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
    // `count_table` silently ignores `ns` when the legacy table has
    // no namespace column. Surface that in the report so an operator
    // who passed a namespace filter knows the legacy side is a
    // global count, not a scoped one.
    let legacy_ns_filter_applied = ns.is_none() || spec.legacy_has_namespace;
    // When the filter was NOT applied to the legacy side but WAS
    // applied to the unified side, raw `delta` is meaningless
    // (legacy is global, unified is scoped). Hold `ok` true only
    // when the filter is consistent across both sides OR no filter
    // was requested.
    let ok = if !legacy_ns_filter_applied {
        // Asymmetric filter: caller asked for ns scoping but legacy
        // side can't honor it. Don't fail the check on this row —
        // it would force every ns-scoped CI run to fail for these
        // three drivers. Surface the asymmetry via the flag instead.
        true
    } else if spec.merge_semantics {
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
        legacy_ns_filter_applied,
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

// ─────────────────────────────────────────────────────────────────────
// I2 — Audit row consistency
// ─────────────────────────────────────────────────────────────────────

/// Scan every completed (`finished_at IS NOT NULL`) row in
/// `backfill_runs` and report any whose counters violate the sum
/// invariant `rows_read == rows_inserted + rows_skipped_existing +
/// rows_failed`.
///
/// In-progress runs (NULL `finished_at`) are skipped on purpose. A
/// driver mid-execution will transiently report partial counts; only
/// finished rows are guaranteed-final and therefore checkable.
///
/// SQL-side filter rather than fetch-then-filter so a large
/// `backfill_runs` history stays cheap.
fn check_audit_consistency(conn: &Connection) -> rusqlite::Result<Vec<AuditViolation>> {
    let mut stmt = conn.prepare(
        "SELECT run_id, legacy_table, rows_read, rows_inserted,
                rows_skipped_existing, rows_failed
         FROM backfill_runs
         WHERE finished_at IS NOT NULL
           AND rows_read <> (rows_inserted + rows_skipped_existing + rows_failed)",
    )?;
    let rows = stmt.query_map([], |row| {
        let rows_read: i64 = row.get(2)?;
        let rows_inserted: i64 = row.get(3)?;
        let rows_skipped_existing: i64 = row.get(4)?;
        let rows_failed: i64 = row.get(5)?;
        let computed_sum =
            (rows_inserted + rows_skipped_existing + rows_failed).max(0) as u64;
        Ok(AuditViolation {
            run_id: row.get(0)?,
            legacy_table: row.get(1)?,
            rows_read: rows_read.max(0) as u64,
            rows_inserted: rows_inserted.max(0) as u64,
            rows_skipped_existing: rows_skipped_existing.max(0) as u64,
            rows_failed: rows_failed.max(0) as u64,
            computed_sum,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────
// I4 — Content spot-check (deterministic sampling)
// ─────────────────────────────────────────────────────────────────────

/// Drive I4 across every driver that has a spot-check implementation.
///
/// Current coverage: T19 memories→nodes only. Other drivers land in
/// follow-up commits; the dispatch shape makes it easy to add them
/// one at a time without touching the report assembly.
///
/// `opts.spot_check_sample_size == 0` disables the check entirely
/// (returns empty Vec without touching the DB), letting CI dial it
/// down for fast smoke runs.
fn check_content_spot_check(
    conn: &Connection,
    ns: Option<&str>,
    opts: &VerifyOpts,
) -> rusqlite::Result<Vec<ContentMismatch>> {
    if opts.spot_check_sample_size == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    spot_check_memories(conn, ns, opts, &mut out)?;
    spot_check_node_embeddings(conn, ns, opts, &mut out)?;
    // T21/T25 pass-through and T22/T23/T24 merge-semantics
    // checks land in subsequent commits.
    Ok(out)
}

/// Deterministically sample N legacy ids from `table`, optionally
/// restricted to `namespace`. Uses `StdRng::seed_from_u64(seed)` so
/// two runs with the same (DB, seed, namespace) MUST select the
/// same row set.
///
/// Strategy: fetch all eligible ids, then shuffle with the seeded
/// PRNG and take the first N. The all-ids fetch is fine for the
/// current scale (hundreds of thousands of rows fits in memory and
/// the alternative — `ORDER BY RANDOM()` — is non-deterministic).
/// If verifier becomes a hot path on multi-million-row tables, this
/// helper switches to a reservoir sample without changing callers.
fn sample_legacy_ids(
    conn: &Connection,
    table: &str,
    ns: Option<&str>,
    sample_size: usize,
    seed: u64,
) -> rusqlite::Result<Vec<String>> {
    use rand::seq::SliceRandom;
    use rand::SeedableRng;

    let sql = match ns {
        Some(_) => format!("SELECT id FROM {} WHERE namespace = ?", table),
        None => format!("SELECT id FROM {}", table),
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = match ns {
        Some(n) => stmt
            .query_map(rusqlite::params![n], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        None => stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?,
    };

    let mut ids = rows;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    ids.shuffle(&mut rng);
    ids.truncate(sample_size);
    Ok(ids)
}

/// I4 for T19 memories→nodes. For each sampled legacy id, fetch
/// both sides and compare critical fields. The attribute blob is
/// parsed JSON-to-JSON (NOT byte-equal) because writers may serialize
/// keys in different orders and that's not a parity failure.
fn spot_check_memories(
    conn: &Connection,
    ns: Option<&str>,
    opts: &VerifyOpts,
    out: &mut Vec<ContentMismatch>,
) -> rusqlite::Result<()> {
    let ids = sample_legacy_ids(
        conn,
        "memories",
        ns,
        opts.spot_check_sample_size,
        opts.spot_check_seed,
    )?;

    for id in ids {
        // Both sides projected onto the same field set. Selecting
        // separately keeps the JOIN simple and the LHS/RHS clearly
        // attributable when a mismatch fires.
        let legacy: Option<MemoryRow> = conn
            .query_row(
                "SELECT id, namespace, layer, memory_type, content,
                        occurred_at, created_at,
                        working_strength, core_strength, importance,
                        consolidation_count, pinned, source,
                        COALESCE(metadata, '{}')
                 FROM memories WHERE id = ?",
                rusqlite::params![id],
                MemoryRow::from_row,
            )
            .optional()?;
        let unified: Option<MemoryRow> = conn
            .query_row(
                "SELECT id, namespace, layer, memory_type, content,
                        occurred_at, created_at,
                        working_strength, core_strength, importance,
                        consolidation_count, pinned, source,
                        COALESCE(attributes, '{}')
                 FROM nodes WHERE id = ? AND node_kind = 'memory'",
                rusqlite::params![id],
                MemoryRow::from_row,
            )
            .optional()?;

        match (legacy, unified) {
            (None, _) => {
                // Sampled id no longer exists on the legacy side.
                // Shouldn't happen because sample_legacy_ids just
                // selected it, but be defensive against a parallel
                // writer.
                out.push(ContentMismatch {
                    legacy_table: "memories".into(),
                    row_id: id,
                    field: "existence".into(),
                    legacy: "missing".into(),
                    unified: "n/a".into(),
                });
            }
            (Some(_), None) => {
                // Legacy row not yet backfilled. I1 catches this on
                // count, but I4 surfaces it on a per-row basis with
                // the offending id named.
                out.push(ContentMismatch {
                    legacy_table: "memories".into(),
                    row_id: id,
                    field: "existence".into(),
                    legacy: "present".into(),
                    unified: "missing".into(),
                });
            }
            (Some(l), Some(u)) => compare_memory_rows(&l, &u, out),
        }
    }
    Ok(())
}

/// Projection of a memory row shared by the legacy and unified
/// SELECTs. Identical schema by construction — the SELECTs above
/// alias compatible columns.
struct MemoryRow {
    id: String,
    namespace: String,
    layer: String,
    memory_type: String,
    content: String,
    occurred_at: Option<f64>,
    created_at: f64,
    working_strength: f64,
    core_strength: f64,
    importance: f64,
    consolidation_count: i64,
    pinned: i64,
    source: String,
    attributes_json: String,
}

impl MemoryRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(MemoryRow {
            id: row.get(0)?,
            namespace: row.get(1)?,
            layer: row.get(2)?,
            memory_type: row.get(3)?,
            content: row.get(4)?,
            occurred_at: row.get(5)?,
            created_at: row.get(6)?,
            working_strength: row.get(7)?,
            core_strength: row.get(8)?,
            importance: row.get(9)?,
            consolidation_count: row.get(10)?,
            pinned: row.get(11)?,
            source: row.get(12)?,
            attributes_json: row.get(13)?,
        })
    }
}

fn compare_memory_rows(l: &MemoryRow, u: &MemoryRow, out: &mut Vec<ContentMismatch>) {
    let id = &l.id;
    macro_rules! cmp {
        ($field:ident) => {
            if l.$field != u.$field {
                out.push(ContentMismatch {
                    legacy_table: "memories".into(),
                    row_id: id.clone(),
                    field: stringify!($field).into(),
                    legacy: format!("{:?}", l.$field),
                    unified: format!("{:?}", u.$field),
                });
            }
        };
    }
    cmp!(namespace);
    cmp!(layer);
    cmp!(memory_type);
    cmp!(content);
    cmp!(occurred_at);
    cmp!(created_at);
    cmp!(working_strength);
    cmp!(core_strength);
    cmp!(importance);
    cmp!(consolidation_count);
    cmp!(pinned);
    cmp!(source);

    // Attributes round-trip: parse both sides as JSON and compare
    // values. Tolerates key-order differences (legitimate) but
    // catches value drift (real bug).
    let l_attr: serde_json::Value = serde_json::from_str(&l.attributes_json)
        .unwrap_or_else(|_| serde_json::Value::String(l.attributes_json.clone()));
    let u_attr: serde_json::Value = serde_json::from_str(&u.attributes_json)
        .unwrap_or_else(|_| serde_json::Value::String(u.attributes_json.clone()));
    if l_attr != u_attr {
        out.push(ContentMismatch {
            legacy_table: "memories".into(),
            row_id: id.clone(),
            field: "attributes".into(),
            legacy: l_attr.to_string(),
            unified: u_attr.to_string(),
        });
    }
}

// ─────────────────────────────────────────────────────────────────────
// I3 — Idempotency (gated, costly)
// ─────────────────────────────────────────────────────────────────────

/// Re-execute every Phase C driver and report any whose re-run
/// inserts more than zero new rows.
///
/// The contract for a Phase C driver is "running it twice against
/// the same DB is equivalent to running it once" — the second run
/// must observe every legacy row as already-projected and skip it
/// (`rows_skipped_existing == rows_read`, `rows_inserted == 0`).
/// This function is the durable proof of that contract.
///
/// Driver order matches the design.md sec 8.4 ordering: nodes
/// before edges, because edges depend on nodes via FK and a missing
/// node would make an edge driver insert net-new rows on the second
/// pass (which would be flagged here as the I3 violation it is).
fn check_idempotency(
    storage: &mut crate::storage::Storage,
    ns: Option<&str>,
) -> rusqlite::Result<Vec<IdempotencyViolation>> {
    use crate::substrate::backfill::{
        backfill_embeddings_to_node_embeddings, backfill_entities_to_nodes,
        backfill_entity_relations_to_edges, backfill_hebbian_links_to_edges,
        backfill_memories_to_nodes, backfill_memory_entities_to_edges,
        backfill_synthesis_provenance_to_edges,
    };

    let mut violations = Vec::new();
    type DriverFn =
        fn(&mut crate::storage::Storage, Option<&str>) -> rusqlite::Result<crate::substrate::backfill::BackfillRun>;
    let drivers: Vec<(&'static str, DriverFn)> = vec![
        ("memories", backfill_memories_to_nodes),
        ("memory_embeddings", backfill_embeddings_to_node_embeddings),
        ("entities", backfill_entities_to_nodes),
        ("entity_relations", backfill_entity_relations_to_edges),
        ("memory_entities", backfill_memory_entities_to_edges),
        ("hebbian_links", backfill_hebbian_links_to_edges),
        ("synthesis_provenance", backfill_synthesis_provenance_to_edges),
    ];

    for (legacy_table, driver) in drivers {
        let run = driver(storage, ns)?;
        if run.rows_inserted > 0 {
            violations.push(IdempotencyViolation {
                legacy_table: legacy_table.to_string(),
                rows_inserted_on_rerun: run.rows_inserted,
                rows_read_on_rerun: run.rows_read,
            });
        }
    }

    Ok(violations)
}

// ─────────────────────────────────────────────────────────────────────
// I4 — T20 spot-check: memory_embeddings → node_embeddings
// ─────────────────────────────────────────────────────────────────────

/// Sample N legacy compound keys from `table`, optionally restricted
/// to a namespace via JOIN to `memories`. Used by T20 whose unique
/// key is `(memory_id, model)` (no scalar PK).
///
/// `ns_join_table_alias` controls the namespace clause shape: when
/// `Some((join_table, fk_col, ns_col))` is provided AND `ns` is set,
/// the query joins to `join_table` on `legacy.{fk_col} =
/// join_table.id` and filters `join_table.{ns_col} = ?`. When `None`,
/// no namespace filtering happens (the legacy table is global).
///
/// Sampling is seeded for reproducibility, same as `sample_legacy_ids`.
fn sample_legacy_compound_keys(
    conn: &Connection,
    table: &str,
    key_cols: (&str, &str),
    ns_join: Option<(&str, &str, &str)>,
    ns: Option<&str>,
    sample_size: usize,
    seed: u64,
) -> rusqlite::Result<Vec<(String, String)>> {
    use rand::seq::SliceRandom;
    use rand::SeedableRng;

    let (k1, k2) = key_cols;
    let sql = match (ns, ns_join) {
        (Some(_), Some((join_table, fk_col, ns_col))) => format!(
            "SELECT t.{k1}, t.{k2} FROM {table} t
             INNER JOIN {join_table} j ON j.id = t.{fk_col}
             WHERE j.{ns_col} = ?"
        ),
        _ => format!("SELECT {k1}, {k2} FROM {table}"),
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(String, String)> = match (ns, ns_join) {
        (Some(n), Some(_)) => stmt
            .query_map(rusqlite::params![n], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        _ => stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?,
    };

    let mut keys = rows;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    keys.shuffle(&mut rng);
    keys.truncate(sample_size);
    Ok(keys)
}

/// Projection for a single memory_embeddings / node_embeddings row.
/// Both sides hydrate into this struct so the comparison is symmetric.
/// `created_at_epoch` is computed: legacy stores RFC3339 TEXT, unified
/// stores REAL; we project both onto f64 seconds-since-epoch (same
/// formula the T20 driver uses).
#[derive(Debug, Clone, PartialEq)]
struct EmbeddingRow {
    memory_id: String,
    model: String,
    dimensions: i64,
    embedding: Vec<u8>,
    created_at_epoch: f64,
}

/// Parse legacy RFC3339 created_at into epoch f64 using the same
/// formula the T20 driver applies. Returns the parsed value plus a
/// `parsed_ok` flag so the spot-check can decide whether to compare
/// timestamps or skip that field (legacy parse failure = driver
/// substituted `utc_now_f64()` which is not reproducible — comparing
/// would always fire).
fn parse_legacy_embedding_created_at(text: &str) -> (f64, bool) {
    match chrono::DateTime::parse_from_rfc3339(text) {
        Ok(dt) => {
            let dt_utc = dt.with_timezone(&chrono::Utc);
            let epoch = dt_utc.timestamp() as f64
                + (dt_utc.timestamp_subsec_nanos() as f64 / 1e9);
            (epoch, true)
        }
        Err(_) => (0.0, false),
    }
}

/// I4 for T20 memory_embeddings → node_embeddings. For each sampled
/// `(memory_id, model)` key, fetch both sides and compare scalar
/// fields + the embedding BLOB byte-equal. `created_at` is compared
/// at f64 precision but skipped when the legacy parse fails (the
/// driver substitutes `utc_now()` on parse failure, which is not a
/// parity bug, just unrecoverable data).
fn spot_check_node_embeddings(
    conn: &Connection,
    ns: Option<&str>,
    opts: &VerifyOpts,
    out: &mut Vec<ContentMismatch>,
) -> rusqlite::Result<()> {
    use rusqlite::OptionalExtension;

    let keys = sample_legacy_compound_keys(
        conn,
        "memory_embeddings",
        ("memory_id", "model"),
        Some(("memories", "memory_id", "namespace")),
        ns,
        opts.spot_check_sample_size,
        opts.spot_check_seed,
    )?;

    for (memory_id, model) in keys {
        let legacy: Option<EmbeddingRow> = conn
            .query_row(
                "SELECT memory_id, model, dimensions, embedding, created_at
                 FROM memory_embeddings WHERE memory_id = ? AND model = ?",
                rusqlite::params![memory_id, model],
                |row| {
                    let created_at_text: String = row.get(4)?;
                    let (epoch, _ok) =
                        parse_legacy_embedding_created_at(&created_at_text);
                    Ok(EmbeddingRow {
                        memory_id: row.get(0)?,
                        model: row.get(1)?,
                        dimensions: row.get(2)?,
                        embedding: row.get(3)?,
                        created_at_epoch: epoch,
                    })
                },
            )
            .optional()?;
        let unified: Option<EmbeddingRow> = conn
            .query_row(
                "SELECT node_id, model, dimensions, embedding, created_at
                 FROM node_embeddings WHERE node_id = ? AND model = ?",
                rusqlite::params![memory_id, model],
                |row| {
                    Ok(EmbeddingRow {
                        memory_id: row.get(0)?,
                        model: row.get(1)?,
                        dimensions: row.get(2)?,
                        embedding: row.get(3)?,
                        created_at_epoch: row.get(4)?,
                    })
                },
            )
            .optional()?;

        let row_id = format!("{memory_id}|{model}");
        match (legacy, unified) {
            (None, _) => {
                out.push(ContentMismatch {
                    legacy_table: "memory_embeddings".into(),
                    row_id,
                    field: "existence".into(),
                    legacy: "missing".into(),
                    unified: "n/a".into(),
                });
            }
            (Some(_), None) => {
                out.push(ContentMismatch {
                    legacy_table: "memory_embeddings".into(),
                    row_id,
                    field: "existence".into(),
                    legacy: "present".into(),
                    unified: "missing".into(),
                });
            }
            (Some(l), Some(u)) => {
                compare_embedding_rows(&l, &u, &row_id, out);
            }
        }
    }
    Ok(())
}

/// Field-by-field comparison for two EmbeddingRow projections. Scalar
/// fields compared by value; embedding BLOB compared byte-equal;
/// created_at compared at f64 precision unless the legacy parse
/// failed (epoch=0.0 sentinel from `parse_legacy_embedding_created_at`).
fn compare_embedding_rows(
    l: &EmbeddingRow,
    u: &EmbeddingRow,
    row_id: &str,
    out: &mut Vec<ContentMismatch>,
) {
    macro_rules! cmp_field {
        ($field:ident) => {
            if l.$field != u.$field {
                out.push(ContentMismatch {
                    legacy_table: "memory_embeddings".into(),
                    row_id: row_id.to_string(),
                    field: stringify!($field).into(),
                    legacy: format!("{:?}", l.$field),
                    unified: format!("{:?}", u.$field),
                });
            }
        };
    }
    cmp_field!(memory_id);
    cmp_field!(model);
    cmp_field!(dimensions);

    if l.embedding != u.embedding {
        out.push(ContentMismatch {
            legacy_table: "memory_embeddings".into(),
            row_id: row_id.to_string(),
            field: "embedding".into(),
            legacy: format!("{} bytes (hash {:?})", l.embedding.len(),
                            blake_short(&l.embedding)),
            unified: format!("{} bytes (hash {:?})", u.embedding.len(),
                             blake_short(&u.embedding)),
        });
    }

    // Created_at: skip if legacy parse failed (driver substituted
    // utc_now() — not a parity bug, just irrecoverable). Otherwise
    // compare at f64 epsilon-equality at microsecond precision (the
    // driver formula has ~ns precision but RFC3339 only stores to
    // 9 decimal digits, so 1e-6 is safe).
    let parsed_ok = l.created_at_epoch != 0.0;
    if parsed_ok && (l.created_at_epoch - u.created_at_epoch).abs() > 1e-6 {
        out.push(ContentMismatch {
            legacy_table: "memory_embeddings".into(),
            row_id: row_id.to_string(),
            field: "created_at_epoch".into(),
            legacy: format!("{:.9}", l.created_at_epoch),
            unified: format!("{:.9}", u.created_at_epoch),
        });
    }
}

/// 8-byte fingerprint of a BLOB for human-readable mismatch
/// messages. Not cryptographic; used only so operators can tell at
/// a glance "the blobs are different" without printing 6 KB hex.
fn blake_short(bytes: &[u8]) -> [u8; 8] {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish().to_le_bytes()
}
