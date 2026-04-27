//! # Bi-temporal projection (`task:retr-impl-factual-bitemporal`)
//!
//! Cross-cutting helper applied wherever a plan surfaces edges (Factual,
//! Episodic, Hybrid). The projection rule is centralized here so it stays
//! consistent across plans — design §4.6, GOAL-3.4, GOAL-3.5, GUARD-3.
//!
//! ## Rule (design §4.6)
//!
//! - **Default ([`AsOfMode::Now`]):** edges with `invalidated_at IS NULL OR
//!   invalidated_at > now()` are included. Superseded edges
//!   (`invalidated_at <= now()`) are excluded from default results.
//! - **`as-of-T` ([`AsOfMode::At(t)`]):** edges with `valid_from <= T AND
//!   (valid_to IS NULL OR valid_to > T)` are included; the rest are
//!   excluded. Edges that would be "superseded" according to the current
//!   clock but were valid at T are still returned — this is the as-of-T
//!   projection.
//! - **`include_superseded` ([`AsOfMode::IncludeSuperseded`]):** every edge
//!   is returned, with the projection annotating whether each row is live
//!   or superseded (and, if superseded, when and by whom). This is how
//!   history is accessed for schema-evolution review.
//!
//! ## Terminology bridge (design §4.6)
//!
//! Retrieval's API uses **"superseded"** (the action-verb framing). The
//! storage layer in `v03-graph-layer` §4.1 names the column `invalidated_at`
//! / `invalidated_by` (the state framing). Both refer to the same rows: an
//! [`Edge`] with non-`None` `invalidated_at` is a superseded edge. The
//! translation happens at the call site:
//! `GraphQuery.include_superseded == true` ⇔
//! `GraphStore::edges_of(.., include_invalidated = true)`.
//!
//! ## GUARD-3 (hard)
//!
//! Bi-temporal invalidation **never erases**. A superseded edge is *always*
//! retrievable via [`AsOfMode::IncludeSuperseded`] or [`AsOfMode::At`],
//! forever. The projection helper does not delete; it only filters /
//! annotates. A property test in `tests/` (and the cross-cutting
//! `task:retr-test-determinism-routing-accuracy` acceptance gate) verifies
//! that after N supersession operations, the full history is still
//! queryable.
//!
//! ## What this module does NOT do
//!
//! - It does **not** issue the SQL query. The storage-layer filter
//!   (WHERE-clause projection at the edge query) lives in
//!   `crate::graph::store::SqliteGraphStore::edges_of` /
//!   `edges_as_of`. This module operates on the resulting `Vec<Edge>` for
//!   the cases where (a) callers fetched with `include_invalidated = true`
//!   and need to apply the as-of-T projection in memory, or (b) tests need
//!   a pure-function projector.
//! - It does **not** mutate edges. Projection is read-only.

use chrono::{DateTime, Utc};

use crate::graph::edge::Edge;

// ---------------------------------------------------------------------------
// Modes
// ---------------------------------------------------------------------------

/// Bi-temporal projection mode (design §4.6).
///
/// Constructed by the plan executor from `GraphQuery.as_of` /
/// `GraphQuery.include_superseded`. The translation rule is encoded in
/// [`AsOfMode::from_query`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsOfMode {
    /// Default: current-state. Live edges only (`invalidated_at IS NULL` or
    /// strictly in the future relative to `now`).
    Now {
        /// Wall-clock instant against which "live" is judged. Pinned by
        /// `query_time` (§5.4) for reproducibility — never sampled here.
        now: DateTime<Utc>,
    },
    /// `as-of-T`: edges valid at instant `T`. Live status at `now` is
    /// irrelevant; only the edge's own `valid_from` / `valid_to` matter.
    At(DateTime<Utc>),
    /// History view: every edge is returned. Superseded rows are annotated
    /// (see [`ProjectedEdge::is_live`] / [`ProjectedEdge::superseded_at`]).
    /// `now` is still required so live/superseded status can be reported
    /// alongside the row.
    IncludeSuperseded {
        /// Wall-clock instant for live/superseded annotation only.
        now: DateTime<Utc>,
    },
}

impl AsOfMode {
    /// Translate `(GraphQuery.as_of, GraphQuery.include_superseded,
    /// GraphQuery.query_time)` into the projection mode.
    ///
    /// Precedence (design §6.2 + §4.6):
    ///
    /// 1. `include_superseded == true` → [`AsOfMode::IncludeSuperseded`]
    ///    (history dominates as-of).
    /// 2. `as_of = Some(t)` → [`AsOfMode::At(t)`].
    /// 3. Otherwise → [`AsOfMode::Now { now: query_time.unwrap_or(Utc::now()) }`].
    ///
    /// Note that `query_time` is the reproducibility pin (§5.4); when set
    /// it replaces `now()` everywhere in retrieval — including this
    /// projection. Callers in tests pass an explicit `query_time` so the
    /// helper never samples the system clock.
    pub fn from_query(
        as_of: Option<DateTime<Utc>>,
        include_superseded: bool,
        query_time: DateTime<Utc>,
    ) -> Self {
        if include_superseded {
            AsOfMode::IncludeSuperseded { now: query_time }
        } else if let Some(t) = as_of {
            AsOfMode::At(t)
        } else {
            AsOfMode::Now { now: query_time }
        }
    }

    /// Wall-clock instant the projection considers "now" for live/superseded
    /// annotation. For [`AsOfMode::At`], the "now" of the projection IS the
    /// pinned time — there is no separate clock.
    pub fn now(&self) -> DateTime<Utc> {
        match self {
            AsOfMode::Now { now } | AsOfMode::IncludeSuperseded { now } => *now,
            AsOfMode::At(t) => *t,
        }
    }

    /// `true` iff this mode wants history (superseded rows are not filtered
    /// out). Used by the storage call site to pass `include_invalidated`.
    pub fn wants_history(&self) -> bool {
        matches!(
            self,
            AsOfMode::IncludeSuperseded { .. } | AsOfMode::At(_)
        )
    }
}

// ---------------------------------------------------------------------------
// Projected output
// ---------------------------------------------------------------------------

/// A projected edge — the projection rule's output row.
///
/// Wraps the raw [`Edge`] alongside the projection's verdict on whether the
/// edge is live (or, for [`AsOfMode::IncludeSuperseded`], when/why it became
/// superseded). Plans surface these wrappers so explain-trace
/// (`task:retr-impl-explain-trace`) can record the projection details
/// without re-deriving them from the underlying edge.
#[derive(Debug, Clone)]
pub struct ProjectedEdge {
    /// The raw edge as stored.
    pub edge: Edge,
    /// `true` iff the edge is live (or as-of-valid) under the active
    /// projection mode. Always `true` for filtered-out modes (since
    /// filtered-out rows are not returned at all); meaningful for the
    /// [`AsOfMode::IncludeSuperseded`] history view.
    pub is_live: bool,
    /// `Some(ts)` iff this edge was superseded at or before the projection
    /// reference instant (for [`AsOfMode::IncludeSuperseded`]) — used by
    /// callers and explain-trace to surface schema-evolution history.
    /// `None` for live edges.
    pub superseded_at: Option<DateTime<Utc>>,
    /// `Some(id)` iff this edge was succeeded by another edge in the
    /// invalidation chain (`invalidated_by`). `None` for live edges, and
    /// for orphan-superseded edges (where the successor is unknown — a
    /// state allowed by the relaxed §3.2 invariant in `graph::edge`).
    pub superseded_by: Option<uuid::Uuid>,
}

impl ProjectedEdge {
    fn live(edge: Edge) -> Self {
        Self {
            edge,
            is_live: true,
            superseded_at: None,
            superseded_by: None,
        }
    }

    fn superseded(edge: Edge) -> Self {
        let superseded_at = edge.invalidated_at;
        let superseded_by = edge.invalidated_by;
        Self {
            edge,
            is_live: false,
            superseded_at,
            superseded_by,
        }
    }
}

// ---------------------------------------------------------------------------
// Projection helper
// ---------------------------------------------------------------------------

/// Apply the bi-temporal projection rule to a raw edge slice.
///
/// **Inputs.** A `Vec<Edge>` fetched from the storage layer. Callers SHOULD
/// fetch with `include_invalidated = mode.wants_history()` — the projection
/// is a pure-function refinement on top of that.
///
/// **Outputs.** A `Vec<ProjectedEdge>` filtered + annotated per the mode:
///
/// - [`AsOfMode::Now`]: keeps only live edges (`invalidated_at` is `None`
///   *or* strictly greater than `now`). Annotation is trivially live.
/// - [`AsOfMode::At(t)`]: keeps edges whose `valid_from <= t AND
///   (valid_to IS NULL OR valid_to > t)`. Annotation is live (the row
///   *was* valid at T regardless of current invalidation status — that is
///   the defining property of as-of-T).
/// - [`AsOfMode::IncludeSuperseded`]: keeps **every** edge. Each is
///   annotated as live (if `invalidated_at` is `None` or `> now`) or
///   superseded (with `superseded_at` / `superseded_by` carried through).
///
/// **GUARD-3.** This function is non-destructive: it never deletes a row,
/// it only filters / annotates. The full history is reachable via
/// [`AsOfMode::IncludeSuperseded`] or [`AsOfMode::At`] and that is
/// orthogonal to whether the storage call returned superseded rows.
///
/// **Determinism.** Pure function over inputs. No clock sampling — `now`
/// must be carried in the [`AsOfMode`].
pub fn project_edges(edges: Vec<Edge>, mode: AsOfMode) -> Vec<ProjectedEdge> {
    let now = mode.now();
    let mut out = Vec::with_capacity(edges.len());
    for edge in edges {
        match mode {
            AsOfMode::Now { .. } => {
                if is_live_at(&edge, now) {
                    out.push(ProjectedEdge::live(edge));
                }
            }
            AsOfMode::At(t) => {
                if was_valid_at(&edge, t) {
                    // GOAL-3.4: as-of-T projection — surface the row as
                    // live for the caller, even if currently superseded.
                    out.push(ProjectedEdge::live(edge));
                }
            }
            AsOfMode::IncludeSuperseded { .. } => {
                if is_live_at(&edge, now) {
                    out.push(ProjectedEdge::live(edge));
                } else {
                    out.push(ProjectedEdge::superseded(edge));
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Predicates
// ---------------------------------------------------------------------------

/// `true` iff `edge` is live at instant `now`. "Live" = not yet superseded
/// according to `invalidated_at`. Edges with `invalidated_at == None` are
/// trivially live. Edges with `invalidated_at = Some(ts)` are live iff
/// `ts > now` (i.e. they will be superseded *later*).
fn is_live_at(edge: &Edge, now: DateTime<Utc>) -> bool {
    match edge.invalidated_at {
        None => true,
        Some(inv) => inv > now,
    }
}

/// `true` iff `edge` was valid at instant `t` per its bi-temporal
/// validity window. `valid_from <= t AND (valid_to IS NULL OR valid_to > t)`.
///
/// `valid_from = None` is treated as **always-valid-from** (open-ended
/// past); `valid_to = None` is treated as **still-valid** (open-ended
/// future). This matches the design §4.6 prose ("edges with `valid_from
/// <= T AND (valid_to IS NULL OR valid_to > T)`") plus the natural
/// reading of an unset lower bound.
fn was_valid_at(edge: &Edge, t: DateTime<Utc>) -> bool {
    let from_ok = match edge.valid_from {
        None => true,
        Some(from) => from <= t,
    };
    let to_ok = match edge.valid_to {
        None => true,
        Some(to) => to > t,
    };
    from_ok && to_ok
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::EdgeEnd;
    use crate::graph::schema::Predicate;
    use chrono::TimeZone;
    use uuid::Uuid;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().unwrap()
    }

    fn edge_at(
        valid_from: Option<DateTime<Utc>>,
        valid_to: Option<DateTime<Utc>>,
        invalidated_at: Option<DateTime<Utc>>,
    ) -> Edge {
        let mut e = Edge::new(
            Uuid::new_v4(),
            Predicate::proposed("test_pred"),
            EdgeEnd::Entity { id: Uuid::new_v4() },
            valid_from,
            ts(1_000),
        );
        e.valid_to = valid_to;
        e.invalidated_at = invalidated_at;
        e
    }

    #[test]
    fn from_query_precedence_history_dominates() {
        // include_superseded=true with as_of=Some(_) → IncludeSuperseded wins.
        let mode =
            AsOfMode::from_query(Some(ts(500)), /* include_superseded */ true, ts(1_000));
        assert!(matches!(mode, AsOfMode::IncludeSuperseded { .. }));
    }

    #[test]
    fn from_query_as_of_when_no_history() {
        let mode = AsOfMode::from_query(Some(ts(500)), false, ts(1_000));
        assert_eq!(mode, AsOfMode::At(ts(500)));
    }

    #[test]
    fn from_query_default_now() {
        let mode = AsOfMode::from_query(None, false, ts(1_000));
        assert_eq!(mode, AsOfMode::Now { now: ts(1_000) });
    }

    #[test]
    fn now_mode_filters_superseded() {
        // 3 edges: live, superseded-in-past, superseded-in-future.
        let live = edge_at(Some(ts(100)), None, None);
        let past_sup = edge_at(Some(ts(100)), None, Some(ts(500))); // superseded before now=1000
        let future_sup = edge_at(Some(ts(100)), None, Some(ts(2_000))); // not yet superseded at now=1000
        let projected = project_edges(
            vec![live.clone(), past_sup, future_sup.clone()],
            AsOfMode::Now { now: ts(1_000) },
        );
        assert_eq!(projected.len(), 2);
        assert!(projected.iter().all(|p| p.is_live));
        let ids: Vec<_> = projected.iter().map(|p| p.edge.id).collect();
        assert!(ids.contains(&live.id));
        assert!(ids.contains(&future_sup.id));
    }

    #[test]
    fn at_mode_keeps_edges_valid_at_t() {
        // valid window [100, 800); at T=500 → live; at T=900 → out.
        let bounded = edge_at(Some(ts(100)), Some(ts(800)), None);
        let projected_in =
            project_edges(vec![bounded.clone()], AsOfMode::At(ts(500)));
        assert_eq!(projected_in.len(), 1);
        assert!(projected_in[0].is_live);

        let projected_out =
            project_edges(vec![bounded.clone()], AsOfMode::At(ts(900)));
        assert!(projected_out.is_empty());

        // exact boundary: valid_to is exclusive (`> t`), so at t == valid_to the edge is OUT.
        let projected_boundary =
            project_edges(vec![bounded], AsOfMode::At(ts(800)));
        assert!(projected_boundary.is_empty());
    }

    #[test]
    fn at_mode_resurrects_currently_superseded() {
        // Edge valid in [100, 800), currently superseded (invalidated_at=900).
        // Querying as-of T=500 still returns it as live — that's the GOAL-3.4
        // resurrection property.
        let edge = edge_at(Some(ts(100)), Some(ts(800)), Some(ts(900)));
        let projected = project_edges(vec![edge.clone()], AsOfMode::At(ts(500)));
        assert_eq!(projected.len(), 1);
        assert!(projected[0].is_live, "as-of-T must report live regardless of current invalidation");
        assert_eq!(projected[0].edge.id, edge.id);
    }

    #[test]
    fn include_superseded_returns_all_with_annotation() {
        let live = edge_at(Some(ts(100)), None, None);
        let past_sup = edge_at(Some(ts(100)), None, Some(ts(500)));
        let projected = project_edges(
            vec![live.clone(), past_sup.clone()],
            AsOfMode::IncludeSuperseded { now: ts(1_000) },
        );
        assert_eq!(projected.len(), 2);
        let live_proj = projected.iter().find(|p| p.edge.id == live.id).unwrap();
        assert!(live_proj.is_live);
        assert!(live_proj.superseded_at.is_none());

        let sup_proj = projected.iter().find(|p| p.edge.id == past_sup.id).unwrap();
        assert!(!sup_proj.is_live);
        assert_eq!(sup_proj.superseded_at, Some(ts(500)));
    }

    #[test]
    fn unbounded_validity_treated_as_open() {
        // valid_from=None and valid_to=None → matches every as-of-T query.
        let unbounded = edge_at(None, None, None);
        for t in [ts(0), ts(500), ts(10_000)] {
            let projected = project_edges(vec![unbounded.clone()], AsOfMode::At(t));
            assert_eq!(projected.len(), 1, "unbounded edge missing at T={t:?}");
        }
    }

    #[test]
    fn guard_3_history_always_recoverable() {
        // GUARD-3 (hard): N supersession ops, full history still queryable.
        // Build a chain of 5 superseded edges + 1 live edge.
        let mut edges = vec![];
        for i in 0..5 {
            edges.push(edge_at(
                Some(ts(100 + i * 10)),
                None,
                Some(ts(200 + i * 10)),
            ));
        }
        let live = edge_at(Some(ts(500)), None, None);
        edges.push(live.clone());

        // Default Now-mode hides superseded.
        let now_view = project_edges(edges.clone(), AsOfMode::Now { now: ts(1_000) });
        assert_eq!(now_view.len(), 1, "Now mode should hide superseded chain");
        assert_eq!(now_view[0].edge.id, live.id);

        // IncludeSuperseded recovers every row.
        let history_view =
            project_edges(edges, AsOfMode::IncludeSuperseded { now: ts(1_000) });
        assert_eq!(history_view.len(), 6);
        let recovered = history_view.iter().filter(|p| !p.is_live).count();
        assert_eq!(recovered, 5, "all 5 superseded edges must be recoverable");
    }

    #[test]
    fn wants_history_flag_matches_modes() {
        assert!(!AsOfMode::Now { now: ts(0) }.wants_history());
        assert!(AsOfMode::At(ts(0)).wants_history());
        assert!(AsOfMode::IncludeSuperseded { now: ts(0) }.wants_history());
    }
}
