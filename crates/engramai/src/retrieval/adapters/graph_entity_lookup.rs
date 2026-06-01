//! `GraphEntityLookup` — classifier's [`EntityLookup`] backed by the
//! v0.3 graph layer.
//!
//! ## Purpose (ISS-171)
//!
//! The Stage-1 classifier's entity signal is computed by
//! [`score_entity`](crate::retrieval::classifier::heuristic::score_entity),
//! which tokenizes the query and asks an [`EntityLookup`] whether each
//! token resolves to a known entity. Prior to ISS-171 the production
//! caller at `retrieval/api.rs` hard-coded
//! [`HeuristicClassifier::with_null_lookup`](crate::retrieval::classifier::HeuristicClassifier::with_null_lookup),
//! so `score_entity` was 0.0 for **every** query and the `Factual`
//! intent was architecturally unreachable. `route_stage1` always took
//! the `strong.len() == 0` branch and downgraded to `Associative`.
//!
//! This adapter is the "wire it to the real graph" follow-up promised
//! in the doc comment at `classifier/heuristic.rs:115-118`
//! (`task:retr-impl-classifier-core`).
//!
//! ## Design
//!
//! The classifier holds an `Arc<dyn EntityLookup>`. That forces this
//! adapter to be `'static + Send + Sync`, so it owns a clone of the
//! same `Arc<Mutex<SqliteGraphStore<'static>>>` that
//! [`Memory::with_graph_read`](crate::memory::Memory::with_graph_read)
//! locks internally — we cannot hold a `&'a dyn GraphRead` here.
//!
//! On each `lookup(token)` call:
//!
//! 1. The token is normalized via
//!    [`normalize_alias`](crate::graph::entity::normalize_alias)
//!    (trim, lowercase, NFKC) for symmetry with the writer path. Empty
//!    or all-stopword tokens short-circuit to `EntityMatch::None`.
//! 2. We acquire the graph mutex once per call. If poisoned, return
//!    `EntityMatch::None` — the classifier degrades to the
//!    pre-ISS-171 behavior rather than crashing the query.
//! 3. We iterate up to `MAX_NAMESPACES_SCANNED` (default 32)
//!    namespaces in deterministic order
//!    ([`GraphRead::list_namespaces`] returns sorted distinct).
//! 4. For each namespace we call
//!    [`GraphRead::search_candidates`] with
//!    `mention_embedding=None, top_k=1`. Per the store's
//!    optimization note (§4.2 in `graph/store.rs:2080`), omitting
//!    the embedding turns this into a single indexed point lookup
//!    (`graph_entity_aliases.normalized` → optional row in
//!    `graph_entities`). Microseconds.
//! 5. Map the result to [`EntityMatch`]: `Exact` if the candidate's
//!    `canonical_name` normalizes to the same string as the token,
//!    `Alias` otherwise (i.e. the alias-exact path hit but the
//!    canonical name differs — e.g. token "caroline" hits alias for
//!    entity "Caroline Doyle").
//! 6. Early-exit on the first `Exact` match — `score_entity` caps at
//!    1.0 and won't improve from further scanning.
//!
//! ## Why scan all namespaces
//!
//! `GraphQuery` carries an optional `namespace`. When it's `None`
//! the retrieval path resolves to `"default"` (see
//! `api.rs:638-642`), but the actual entities the user cares about
//! may live in per-conversation namespaces (`conv-26`, `conv-44` for
//! LoCoMo) populated by ingestion. The classifier runs **before**
//! the query's namespace is plumbed into plan adapters; scoping to
//! `"default"` here would silently re-introduce the "Factual
//! unreachable" bug for any multi-namespace database.
//!
//! `GraphEntityResolver` makes the same call for the same reason
//! (`graph_entity_resolver.rs` lines on "Why scan all namespaces").
//! We mirror its `MAX_NAMESPACES_SCANNED=32` cap for cost safety.
//!
//! ## Fuzzy
//!
//! [`EntityMatch::Fuzzy`] is **never** returned. The v0
//! `search_candidates` driver has no fuzzy path — only alias-exact
//! (normalized equality). Fuzzy alias matching is tracked
//! separately as ISS-170 and would slot into this adapter without
//! changing the trait surface.
//!
//! ## Determinism
//!
//! The `EntityLookup` trait contract requires pure
//! same-store-same-token → same-result behavior. We achieve this
//! because:
//! - `list_namespaces()` returns sorted-distinct
//!   (`graph/store.rs:3759`).
//! - Within a namespace, `search_candidates(top_k=1)` returns the
//!   highest-scoring single row using deterministic tie-breaks.
//! - We early-exit on `Exact` rather than continuing — that's
//!   deterministic because we visit namespaces in a fixed order.
//!
//! ## Failure handling
//!
//! Any `GraphError` from the store or a poisoned mutex returns
//! `EntityMatch::None` for that token (logged at `warn!`). This
//! preserves the existing "no graph populated" behavior and keeps
//! the classifier on the happy path. The retrieval orchestrator
//! sees a 0.0 entity signal and routes to `Associative` — exactly
//! the pre-ISS-171 fallback. Better to under-fire than to crash
//! the whole query because a transient SQL error fell out of an
//! entity check.

use std::sync::{Arc, Mutex};

use crate::graph::entity::normalize_alias;
use crate::graph::store::{CandidateQuery, GraphRead, SqliteGraphStore};
use crate::retrieval::classifier::heuristic::{EntityLookup, EntityMatch};

/// Hard cap on namespaces visited per `lookup()` call. Matches the cap
/// in [`GraphEntityResolver`](super::graph_entity_resolver) so the two
/// scan the same horizon. With one indexed point lookup per namespace
/// (alias-exact only, no embedding scan), 32 lookups per token stay
/// well inside the classifier's per-query budget even when the
/// database holds dozens of conversational namespaces.
const MAX_NAMESPACES_SCANNED: usize = 32;

/// Single-row top-k for the alias-exact probe. `score_entity` only
/// needs "is there a hit?" — pulling more rows wastes work.
const PER_NAMESPACE_TOP_K: usize = 1;

/// `EntityLookup` implementation that scans the v0.3 graph entity
/// index. See module docs for the full contract.
pub struct GraphEntityLookup {
    graph: Arc<Mutex<SqliteGraphStore<'static>>>,
}

impl GraphEntityLookup {
    /// Construct from the same `Arc<Mutex<SqliteGraphStore>>` that
    /// `Memory::graph_store` holds. The classifier wraps this in
    /// `Arc<dyn EntityLookup>` and calls `lookup()` once per query
    /// token.
    pub fn new(graph: Arc<Mutex<SqliteGraphStore<'static>>>) -> Self {
        Self { graph }
    }
}

impl EntityLookup for GraphEntityLookup {
    fn lookup(&self, token: &str) -> EntityMatch {
        // Normalize for symmetry with the writer path. Empty tokens
        // (would normalize to "") can't possibly hit and shortcut
        // here — saves the namespace list + SQL prep.
        let normalized = normalize_alias(token);
        if normalized.is_empty() {
            return EntityMatch::None;
        }

        // Lock once per call. Poisoned mutex degrades silently to
        // None — see module-level "Failure handling" doc.
        let guard = match self.graph.lock() {
            Ok(g) => g,
            Err(_) => {
                log::warn!(
                    target: "engramai::retrieval::classifier",
                    "GraphEntityLookup: graph store mutex poisoned; \
                     returning EntityMatch::None"
                );
                return EntityMatch::None;
            }
        };
        let graph: &dyn GraphRead = &*guard;

        let namespaces = match graph.list_namespaces() {
            Ok(ns) => ns,
            Err(e) => {
                log::warn!(
                    target: "engramai::retrieval::classifier",
                    "GraphEntityLookup: list_namespaces failed: {e}; \
                     returning EntityMatch::None"
                );
                return EntityMatch::None;
            }
        };

        let mut best = EntityMatch::None;

        for ns in namespaces.into_iter().take(MAX_NAMESPACES_SCANNED) {
            // Build the query inside the loop so each iteration owns its
            // namespace string. `mention_text` is the *raw* token —
            // `search_candidates` normalizes internally. `mention_embedding`
            // = None skips the brute-force embedding scan (store §4.2
            // optimization note) and turns this into one indexed point
            // lookup.
            // `now` and `recency_window` only affect the optional
            // recency-decay score in `CandidateMatch.recency_score`. We
            // don't read that field — we only care whether the alias
            // path fired (`alias_match` / candidate present). Use a
            // fixed sentinel `0.0` so the lookup is deterministic and
            // doesn't drift with wall-clock.
            let q = CandidateQuery {
                mention_text: token.to_string(),
                mention_embedding: None,
                kind_filter: None,
                namespace: ns,
                top_k: PER_NAMESPACE_TOP_K,
                recency_window: None,
                now: 0.0,
            };
            let hits = match graph.search_candidates(&q) {
                Ok(h) => h,
                Err(e) => {
                    log::warn!(
                        target: "engramai::retrieval::classifier",
                        "GraphEntityLookup: search_candidates failed \
                         (namespace={:?}): {e}; skipping namespace",
                        q.namespace
                    );
                    continue;
                }
            };
            let Some(hit) = hits.into_iter().next() else {
                continue;
            };

            // search_candidates with mention_embedding=None will only
            // return a row if the alias-exact path fired (no embedding
            // scan was performed). Therefore any hit at this point is
            // at minimum an alias match. Distinguish Exact (token ==
            // canonical name) from Alias (alias matched a non-canonical
            // form) by normalizing both sides.
            let canonical_norm = normalize_alias(&hit.canonical_name);
            let this_match = if canonical_norm == normalized {
                EntityMatch::Exact
            } else {
                EntityMatch::Alias
            };

            if rank(this_match) > rank(best) {
                best = this_match;
            }
            if matches!(best, EntityMatch::Exact) {
                break; // can't improve on Exact
            }
        }

        best
    }
}

/// Rank for "which match is stronger" tie-break across namespaces.
/// Order mirrors `score_entity`'s 1.0 / 0.8 / 0.5 / 0.0 weighting.
fn rank(m: EntityMatch) -> u8 {
    match m {
        EntityMatch::Exact => 3,
        EntityMatch::Alias => 2,
        EntityMatch::Fuzzy => 1,
        EntityMatch::None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::entity::{Entity, EntityKind};
    use crate::graph::store::GraphWrite;
    use chrono::Utc;
    use rusqlite::Connection;

    /// Build an in-memory `SqliteGraphStore<'static>` and pre-populate
    /// it with one or more `(namespace, canonical_name, alias)` rows.
    /// Returns the `Arc<Mutex<...>>` in the shape
    /// [`GraphEntityLookup::new`] consumes.
    ///
    /// The connection is `Box::leak`ed to obtain `'static`, mirroring
    /// the production path in
    /// [`Memory::with_pipeline_pool`](crate::memory::Memory::with_pipeline_pool)
    /// (`memory.rs:343`). Tests are expected to leak — they run in
    /// isolated processes and the leak lives only for the test's
    /// duration.
    fn populated_store(
        rows: &[(&str, &str, Option<&str>)],
    ) -> Arc<Mutex<SqliteGraphStore<'static>>> {
        let conn = Connection::open_in_memory().expect("in-mem conn");
        // Initialize the v0.3 graph schema (the production path does
        // this via `crate::graph::init_graph_tables`; see memory.rs:368).
        crate::graph::init_graph_tables(&conn).expect("init graph schema");
        let leaked: &'static mut Connection = Box::leak(Box::new(conn));

        // Build the store fresh, write each row in its own namespace,
        // re-targeting via `with_namespace` (which consumes self) and
        // collecting back via `std::mem::replace`-free chaining.
        let mut store: SqliteGraphStore<'static> = SqliteGraphStore::new(leaked);
        let now = Utc::now();
        for (ns, canonical, alias) in rows {
            // Switch namespace by consuming and rebinding. We do it
            // through a temporary because `with_namespace` takes
            // `self`. We never observe the store in an in-between
            // state because the rebind is single-statement.
            store = store.with_namespace((*ns).to_string());
            let entity = Entity::new_random_id((*canonical).to_string(), EntityKind::Person, now);
            store.insert_entity(&entity).expect("insert_entity");
            if let Some(a) = alias {
                store
                    .upsert_alias(&normalize_alias(a), a, entity.id, None)
                    .expect("upsert_alias");
            }
        }
        Arc::new(Mutex::new(store))
    }

    #[test]
    fn empty_token_returns_none() {
        let store = populated_store(&[]);
        let lookup = GraphEntityLookup::new(store);
        assert_eq!(lookup.lookup(""), EntityMatch::None);
        assert_eq!(lookup.lookup("   "), EntityMatch::None);
    }

    #[test]
    fn unknown_token_returns_none() {
        let store = populated_store(&[("default", "Caroline", Some("caroline"))]);
        let lookup = GraphEntityLookup::new(store);
        assert_eq!(lookup.lookup("xyzzy"), EntityMatch::None);
    }

    #[test]
    fn exact_canonical_match_returns_exact() {
        // canonical_name = "caroline" so normalize(token) ==
        // normalize(canonical_name) → Exact.
        let store = populated_store(&[("default", "caroline", Some("caroline"))]);
        let lookup = GraphEntityLookup::new(store);
        assert_eq!(lookup.lookup("caroline"), EntityMatch::Exact);
        // Case-insensitive normalization.
        assert_eq!(lookup.lookup("Caroline"), EntityMatch::Exact);
    }

    #[test]
    fn alias_only_match_returns_alias() {
        // Token "caroline" matches the alias, but the canonical_name
        // "Caroline Doyle" doesn't normalize to "caroline" → Alias.
        let store = populated_store(&[("default", "Caroline Doyle", Some("caroline"))]);
        let lookup = GraphEntityLookup::new(store);
        assert_eq!(lookup.lookup("caroline"), EntityMatch::Alias);
    }

    #[test]
    fn cross_namespace_match_is_found() {
        // Entity lives in conv-26, lookup runs across all namespaces.
        // canonical_name "Caroline" normalizes to "caroline" → Exact.
        let store = populated_store(&[("conv-26", "Caroline", Some("caroline"))]);
        let lookup = GraphEntityLookup::new(store);
        assert_eq!(lookup.lookup("caroline"), EntityMatch::Exact);
    }

    #[test]
    fn exact_wins_over_alias_across_namespaces() {
        // ns1 has the token as alias only (canonical = "Caroline Doyle").
        // ns2 has the token as the canonical name.
        // Across all namespaces, Exact must win regardless of order.
        let store = populated_store(&[
            ("ns1", "Caroline Doyle", Some("caroline")),
            ("ns2", "caroline", Some("caroline")),
        ]);
        let lookup = GraphEntityLookup::new(store);
        assert_eq!(lookup.lookup("caroline"), EntityMatch::Exact);
    }

    #[test]
    fn unicode_nfkc_normalization() {
        // NFKC: full-width "Ｃａｒｏｌｉｎｅ" normalizes to "caroline".
        let store = populated_store(&[("default", "Caroline", Some("caroline"))]);
        let lookup = GraphEntityLookup::new(store);
        assert_eq!(
            lookup.lookup("\u{ff23}\u{ff41}\u{ff52}\u{ff4f}\u{ff4c}\u{ff49}\u{ff4e}\u{ff45}"),
            EntityMatch::Exact
        );
    }

    #[test]
    fn deterministic_across_repeated_calls() {
        // Same token + same store → same result, every call.
        let store = populated_store(&[
            ("default", "Alice", Some("alice")),
            ("ns2", "Bob", Some("bob")),
        ]);
        let lookup = GraphEntityLookup::new(store);
        for _ in 0..10 {
            assert_eq!(lookup.lookup("alice"), EntityMatch::Exact);
            assert_eq!(lookup.lookup("bob"), EntityMatch::Exact);
            assert_eq!(lookup.lookup("charlie"), EntityMatch::None);
        }
    }

    #[test]
    fn empty_graph_returns_none() {
        // Sanity: lookup against a store with no entities short-circuits
        // cleanly (the namespace list is empty, no SQL probes at all).
        let store = populated_store(&[]);
        let lookup = GraphEntityLookup::new(store);
        assert_eq!(lookup.lookup("anything"), EntityMatch::None);
    }
}
