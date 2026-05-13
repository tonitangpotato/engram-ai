//! `StorageEpisodicStore` — Episodic plan's [`EpisodicMemoryStore`]
//! backed by a `&Storage` handle plus the v0.3 graph for entity-mention
//! lookup.
//!
//! Implements the two trait methods:
//!
//! - `memories_in_window(window, limit)` — pulls the most-recent memories
//!   via [`Storage::fetch_recent`] (cross-namespace, `"*"`) and filters
//!   in-memory by `record.created_at ∈ [window.start, window.end]`.
//! - `memories_mentioning_entities(entities, limit)` — for each entity in
//!   the input list, calls
//!   [`GraphRead::memories_mentioning_entity`] and unions the
//!   memory-id sets, capped at `limit`.
//!
//! ## Why fetch_recent + in-memory filter (not a SQL window query)
//!
//! v0.3 design treats the time-window scan as plan-local: the trait
//! contract demands "memory ids whose valid time intersects window"
//! and the storage row's `created_at` is the v0.2-era proxy for valid
//! time (see `MemoryRecord` field doc — bi-temporal split lands in
//! `task:retr-impl-bitemporal-projection`). Until that split exists,
//! the cheapest, byte-deterministic implementation is:
//!
//! 1. Fetch a generous candidate set (`limit * 4`, capped at
//!    `MAX_FETCH`) ordered newest-first from Storage.
//! 2. Filter to the window.
//! 3. Truncate to `limit`.
//!
//! Step 1 over-fetches because the LoCoMo-26 corpus has memories
//! spanning months and a single conversation tag (e.g. `2026-04-15
//! morning`) might be 80 rows back. Over-fetching by 4× is cheap and
//! keeps the adapter correctness-first; once volume grows we can add a
//! `Storage::fetch_in_window(start, end, limit)` SQL query.
//!
//! ## Why `"*"` cross-namespace
//!
//! The `EpisodicPlanInputs` has no namespace field — same shape as the
//! [`super::graph_entity_resolver::GraphEntityResolver`] caveat. Until
//! a namespace plumbs through `GraphQuery`, the adapter scans across
//! all namespaces (Storage's `fetch_recent` accepts `"*"` for that).
//!
//! ## Send + Sync
//!
//! Trait already requires `Send + Sync` (orchestrator holds `&dyn
//! EpisodicMemoryStore`). `&Storage` is `Sync` because `Storage` wraps
//! `rusqlite::Connection` behind a single shared handle that the
//! orchestrator's `with_graph_read` closure holds for the duration of
//! the call. `&dyn GraphRead + Send + Sync` is the same shape used by
//! the Factual adapter.

use crate::graph::store::GraphRead;
use crate::retrieval::api::EntityId;
use crate::retrieval::plans::episodic::{EpisodicMemoryStore, ResolvedWindow};
use crate::store_api::MemoryId;
use crate::storage::Storage;

/// Hard cap on rows pulled from Storage per window scan. Bounds memory
/// + scan cost: 1 024 rows × ~1 KiB record ≈ 1 MiB which is acceptable
/// for a single-query allocation. Realistic windows return <50 rows.
const MAX_FETCH: usize = 1024;

/// Over-fetch multiplier — request `limit * 4` rows so the in-memory
/// time filter has headroom even if the window is far in the past.
/// 4× tuned against LoCoMo-26: a 20-message question typically lands
/// inside the most-recent ~80 rows.
const OVERFETCH: usize = 4;

/// Storage-backed [`EpisodicMemoryStore`].
///
/// Borrows `&Storage` (for `fetch_recent`) and `&dyn GraphRead` (for
/// the entity-mention secondary lookup). Both lifetimes are tied to a
/// single `Memory::graph_query` call.
pub struct StorageEpisodicStore<'a> {
    pub storage: &'a Storage,
    pub graph: &'a dyn GraphRead,
}

impl<'a> StorageEpisodicStore<'a> {
    pub fn new(
        storage: &'a Storage,
        graph: &'a dyn GraphRead,
    ) -> Self {
        Self { storage, graph }
    }
}

impl<'a> EpisodicMemoryStore for StorageEpisodicStore<'a> {
    fn memories_in_window(
        &self,
        window: &ResolvedWindow,
        limit: usize,
    ) -> Vec<MemoryId> {
        if limit == 0 {
            return Vec::new();
        }

        // Over-fetch capped at MAX_FETCH; saturating_mul keeps us safe
        // against pathological `limit` values from upstream.
        let fetch = limit.saturating_mul(OVERFETCH).min(MAX_FETCH);

        // Cross-namespace fetch (`"*"` per Storage::fetch_recent docs).
        let rows = match self.storage.fetch_recent(fetch, Some("*")) {
            Ok(r) => r,
            // Failure → empty. The Episodic plan surfaces `Empty`,
            // which is the correct behaviour for "storage unavailable"
            // (matches NullEpisodicStore).
            Err(_) => return Vec::new(),
        };

        let mut hits = Vec::with_capacity(limit.min(rows.len()));
        for record in rows {
            if window.contains(record.created_at) {
                hits.push(record.id);
                if hits.len() == limit {
                    break;
                }
            }
        }
        hits
    }

    fn memories_mentioning_entities(
        &self,
        entities: &[EntityId],
        limit: usize,
    ) -> Option<Vec<MemoryId>> {
        if entities.is_empty() || limit == 0 {
            // Trait contract: `None` = "not supported / no filter".
            // Empty input means "caller asked for no filter" → return
            // None so plan skips filtering rather than producing
            // incorrectly-empty output.
            return None;
        }

        // Per-entity quota — split the budget evenly so a single very
        // popular entity can't crowd out the others. +1 for rounding so
        // a 3-entity query with limit=10 gets per_entity=4 (over-fetch
        // is harmless because we dedupe + truncate).
        let per_entity = (limit / entities.len()).max(1) + 1;

        let mut seen: std::collections::HashSet<MemoryId> =
            std::collections::HashSet::new();
        let mut out: Vec<MemoryId> = Vec::with_capacity(limit);

        for entity_id in entities {
            // EntityId = uuid::Uuid (alias).
            let memory_ids = match self.graph.memories_mentioning_entity(*entity_id, per_entity) {
                Ok(v) => v,
                Err(_) => continue, // Per-entity failure non-fatal.
            };
            for mid in memory_ids {
                if seen.insert(mid.clone()) {
                    out.push(mid);
                    if out.len() == limit {
                        return Some(out);
                    }
                }
            }
        }

        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::store::SqliteGraphStore;
    use crate::graph::test_helpers::fresh_conn;
    use chrono::{Duration, Utc};

    fn fresh_storage() -> Storage {
        Storage::new(":memory:").expect("create in-memory storage")
    }

    fn store_memory(storage: &mut Storage, content: &str, ns: &str) -> String {
        use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
        let now = Utc::now();
        let record = MemoryRecord {
            id: uuid::Uuid::new_v4().to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: now,
            occurred_at: None,
            access_times: vec![now],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: String::new(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        };
        storage.add(&record, ns).expect("store");
        record.id
    }

    #[test]
    fn empty_limit_returns_empty() {
        let storage = fresh_storage();
        let mut conn = fresh_conn();
        let graph = SqliteGraphStore::new(&mut conn);
        let adapter = StorageEpisodicStore::new(&storage, &graph);
        let window = ResolvedWindow {
            start: Utc::now() - Duration::days(1),
            end: Utc::now(),
        };
        assert!(adapter.memories_in_window(&window, 0).is_empty());
    }

    #[test]
    fn returns_ids_inside_window() {
        let mut storage = fresh_storage();
        let id_in = store_memory(&mut storage, "hello", "default");
        let mut conn = fresh_conn();
        let graph = SqliteGraphStore::new(&mut conn);
        let adapter = StorageEpisodicStore::new(&storage, &graph);

        let window = ResolvedWindow {
            start: Utc::now() - Duration::hours(1),
            end: Utc::now() + Duration::hours(1),
        };
        let hits = adapter.memories_in_window(&window, 10);
        assert!(
            hits.iter().any(|m| m.as_str() == id_in.as_str()),
            "expected stored id {id_in} in {hits:?}"
        );
    }

    #[test]
    fn excludes_ids_outside_window() {
        let mut storage = fresh_storage();
        let _ = store_memory(&mut storage, "hello", "default");
        let mut conn = fresh_conn();
        let graph = SqliteGraphStore::new(&mut conn);
        let adapter = StorageEpisodicStore::new(&storage, &graph);

        let window = ResolvedWindow {
            start: Utc::now() + Duration::days(365),
            end: Utc::now() + Duration::days(366),
        };
        assert!(adapter.memories_in_window(&window, 10).is_empty());
    }

    #[test]
    fn empty_entities_returns_none() {
        let storage = fresh_storage();
        let mut conn = fresh_conn();
        let graph = SqliteGraphStore::new(&mut conn);
        let adapter = StorageEpisodicStore::new(&storage, &graph);
        assert!(adapter.memories_mentioning_entities(&[], 5).is_none());
    }

    #[test]
    fn unknown_entity_returns_some_empty() {
        let storage = fresh_storage();
        let mut conn = fresh_conn();
        let graph = SqliteGraphStore::new(&mut conn);
        let adapter = StorageEpisodicStore::new(&storage, &graph);
        let unknown: EntityId = uuid::Uuid::new_v4();
        let out = adapter.memories_mentioning_entities(&[unknown], 5);
        assert_eq!(out, Some(Vec::new()));
    }
}
