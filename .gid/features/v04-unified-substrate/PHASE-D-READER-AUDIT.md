# Phase D Reader-Audit — `unified_substrate` read-switch readiness

> **Status:** DRAFT — overnight scoping pass 2026-05-14 ~00:40 EDT
> Author: rustclaw (autonomous overnight session, see `memory/2026-05-14.md`)
> Purpose: enumerate every legacy-table reader in `storage.rs` and classify it against `nodes`/`edges` projection coverage. Surfaces all Phase B/D **field-mapping gaps** in one place so T29.5/.6/.7 + T31 don't discover them one-at-a-time.

## Why this doc exists

T29.4 hebbian readers landed cleanly because `hebbian_links` → `edges WHERE edge_kind='associative'` is a structurally simple projection (source/target/strength/count/timestamp — all primitives, no nested decoding). The session triggering this audit (2026-05-13 → 2026-05-14 overnight) discovered the next reader cohort is **not** that simple:

1. **`row_to_record_impl` gap (memories → nodes):** the `MemoryRecord` decoder reads `contradicts` and `contradicted_by` columns *by name*. Neither column exists on `nodes`, and `insert_memory_node_row` (T12 dual-write helper) does not stamp them into the `attributes` JSON either. `contradicted_by` is load-bearing — confidence.rs:75, confidence.rs:181, and models/actr.rs:92 all branch on `.contradicted_by.is_some()` for confidence + activation penalty. A unified `search_fts` / `fetch_memory_record` would silently lose this signal.
2. **`EntityRecord.entity_type` gap (entities → nodes):** `dual_write_entity_to_nodes` (graph/store.rs:892) maps `EntityKind` to `node_kind` but only as the binary bifurcation `'topic'` vs `'entity'`. The original `EntityKind` variant (Person/Concept/Event/…) is not stamped into `attributes` JSON nor any nodes column. `find_entities`, `entity_stats`, and `EntityRecord.entity_type` consumers can't recover it from unified.

These gaps mean the T12/T13 dual-writes pass **byte-equal parity on the writer side** (which is what their tests asserted) but fail **read parity on the reader side** for non-trivial decoders. The audit below classifies every remaining reader by this lens.

## Classification key

- ✅ **Trivial-safe** — return type is a primitive or id list; no field-decoding required from row. Switching is mechanical (rewrite the SQL, add flag-gate, add cross-axis test if cross-NS-sensitive).
- ⚠ **Decoder-extension needed** — return type carries fields that need re-decoding from nodes/edges plus attributes JSON. Switching requires a new decoder helper (e.g. `row_to_record_from_node_impl`).
- 🔴 **Dual-write gap** — at least one field consumed by the decoder is not stamped into nodes/edges by current dual-write helpers. **Cannot** switch this reader until a Phase B patch lands.
- 🟡 **Conceptual mismatch / re-design** — the SQL pattern doesn't map cleanly (e.g. FTS5 over an aux content table, schema mismatch, etc.). Needs design discussion.

## Memory-side readers

| Function | Returns | Class | Required action |
|---|---|---|---|
| `all` (storage.rs:2245) | `Vec<MemoryRecord>` | 🔴 | needs decoder + dual-write contradicts patch |
| `all_in_namespace` (3356) | `Vec<MemoryRecord>` | 🔴 | needs decoder + dual-write contradicts patch |
| `get_by_ids` (2261) | `Vec<MemoryRecord>` | 🔴 | needs decoder + dual-write contradicts patch |
| `search_fts` (2527) | `Vec<MemoryRecord>` | 🔴 | needs decoder + `nodes_fts` MATCH parity test + dual-write contradicts patch |
| `search_fts_ns` (3296) | `Vec<MemoryRecord>` | 🔴 | as above |
| `fetch_recent` (2562) | `Vec<MemoryRecord>` | 🔴 | as above |
| `search_by_type` (2592) | `Vec<MemoryRecord>` | 🔴 | as above |
| `search_by_type_ns` (2468) | `Vec<MemoryRecord>` | 🔴 | as above |
| `list_superseded` (3241) | `Vec<(MemoryRecord, String)>` | 🔴 | as above |
| `list_deleted` (3725) | `Vec<MemoryRecord>` | 🔴 | as above |
| `fetch_memory_record` (7090, free fn) | `Option<MemoryRecord>` | 🔴 | as above |
| `fetch_memory_record_with_namespace` (7113) | as above | 🔴 | as above |
| `get_memories_by_ids` (6376) | `Vec<MemoryRecord>` | 🔴 | as above |
| `count_memories` (6447) | `usize` | ✅ | `SELECT COUNT FROM nodes WHERE node_kind='memory' AND deleted_at IS NULL` |
| `count_memories_in_namespace` (6601) | `usize` | ✅ | as above + NS filter |
| `count_soft_deleted` (3749) | `usize` | ✅ | `WHERE deleted_at IS NOT NULL` |
| `count_orphan_memories` (6529) | `usize` | ✅ | redefine "orphan" against nodes-equivalent FK |
| `get_namespace` (3058) | `Option<String>` | ✅ | trivial |
| `get_deleted_at` (3759) | `Option<String>` | ✅ | epoch→RFC3339 cast on read (`nodes.deleted_at` is REAL) |
| `get_memory_timestamp` (5770) | `Option<f64>` | ✅ | trivial |
| `get_memory_content_preview` (4984) | `String` | ✅ | trivial |
| `get_memory_ids_since` (6023) | `Vec<String>` | ✅ | trivial |
| `get_orphan_memory_ids` (6547) | `Vec<String>` | ✅ | as count_orphan_memories |
| `get_unenriched_memory_ids` (6149) | `Vec<String>` | ⚠ | depends on `enrichment_attempts` column — not on nodes; may need attributes JSON convention or stay on memories |
| `get_memories_without_entities` (5279) | `Vec<String>` | ⚠ | needs `memory_entities` ↔ unified `edges` parity (T23 partial) |
| `get_memories_without_embeddings` (3802) | `Vec<String>` | ⚠ | needs node_embeddings parity |
| `list_namespaces` (6522) | `Vec<String>` | ✅ | `SELECT DISTINCT namespace FROM nodes` |
| `list_v1_candidates_page` (6998) | paginated v1 candidates | 🟡 | bespoke supersession query, audit separately |
| `get_pending_memory_ids` (6320) | `Vec<String>` | 🟡 | depends on `pending_memories` aux table — not migrated |
| `get_pending_count` (6440) | `usize` | 🟡 | as above |

## Entity-side readers

| Function | Returns | Class | Required action |
|---|---|---|---|
| `get_entity` (4741) | `Option<EntityRecord>` | 🔴 | `EntityRecord.entity_type` not in nodes; needs T13 dual-write patch to stamp `kind_label` into attributes |
| `find_entities` (4654) | `Vec<EntityRecord>` | 🔴 | as above |
| `list_entities` (4786) | `Vec<EntityRecord>` | 🔴 | as above |
| `entity_stats` (4854) | `(usize, usize, usize)` (entities, relations, mem-links) | 🔴 | breakdown by entity_type → kind gap |
| `count_entities` (4763) | `usize` | ✅ | `SELECT COUNT FROM nodes WHERE node_kind IN ('entity','topic')` (note: 'topic' inclusion is a definitional question — see followup) |
| `get_entities_for_memory` (5732) | `Vec<String>` (id list) | ⚠ | depends on `memory_entities` ↔ `edges` (kind='provenance', predicate='mentions') parity — T23 |
| `get_entity_ids_for_memory` (4701) | `Vec<String>` | ⚠ | as above |
| `get_entity_memories` (4710) | `Vec<String>` | ⚠ | as above (reverse direction) |
| `find_entity_overlap` (5782) | `Vec<(String, usize)>` | ⚠ | bulk JOIN of memory_entities — must verify T23 edges projection has the right cardinality |
| `get_related_entities` (4721) | `Vec<(String, String, String)>` | ⚠ | depends on `entity_relations` ↔ `edges` (kind='structural') parity — T22 |

## Embedding-side readers

T20 already dual-writes `memory_embeddings → node_embeddings`. Read side still on legacy.

| Function | Returns | Class | Required action |
|---|---|---|---|
| `get_embedding` (3489) | `Option<Vec<f32>>` | ✅ | switch to `node_embeddings` |
| `get_embedding_for_memory` (5746) | `Option<Vec<f32>>` | ✅ | as above |
| `get_all_embeddings` (3523) | `Vec<(String, Vec<f32>)>` | ✅ | as above |
| `get_embeddings_in_namespace` (3558) | `Vec<(String, Vec<f32>)>` | ✅ | as above |
| `embedding_stats` (3827) | `EmbeddingStats` | ⚠ | depends on JOIN against memories for namespace count — needs node_embeddings + nodes JOIN |
| `find_nearest_embedding` (4922) | `Option<(String, f32)>` | ⚠ | already routes through embedding store; verify |
| `find_all_above_threshold` (4964) | `Vec<(String, f32)>` | ⚠ | as above |

## Triple / synthesis-provenance / misc

| Function | Returns | Class | Required action |
|---|---|---|---|
| `get_triples` (6107) | `Vec<Triple>` | 🟡 | triples table; T23 covers memory_entities split — verify subject/object roles round-trip |
| `get_insight_sources` (5414) | `Vec<ProvenanceRecord>` | 🔴 | synthesis_provenance; T29.2 only partially covered |
| `get_memory_insights` (5445) | `Vec<ProvenanceRecord>` | 🔴 | as above |
| `get_cluster_centroids` (6223) | `Vec<(String, Vec<f32>)>` | 🟡 | KC clusters — needs separate plan |
| `get_dirty_cluster_ids` (6301) | `Vec<String>` | 🟡 | as above |
| `get_cluster_members` (6327) | `Vec<String>` | 🟡 | as above |
| `get_pending_promotions` (1494) | `Vec<PromotionCandidate>` | 🟡 | promotion queue — separate aux table |
| `count_dangling_hebbian` (6538) | `usize` | ✅ | already in unified scope; can re-define against `edges WHERE edge_kind='associative'` |

## Gap → action map

### Gap #1 — `contradicts` / `contradicted_by` lost in memory→nodes dual-write
- **Filing:** ISS-119 (proposed) — "Phase B dual-write loses `contradicts`/`contradicted_by` for memory→nodes; blocks T29.7 hot retrieval switch"
- **Severity:** P1 (load-bearing for confidence + activation; silent quality loss if unified flips on without fix)
- **Fix option A (preferred):** stamp into `attributes` JSON under reserved keys `_legacy_contradicts` and `_legacy_contradicted_by`. New decoder helper `row_to_record_from_node_impl` reads them out. Backfill driver patches existing T19 rows. Minimal schema churn.
- **Fix option B:** add `contradicts` / `contradicted_by` columns to `nodes`. Schema migration. Cleaner long-term but bigger blast radius.
- **Recommendation:** A for unblock-Phase-D; revisit B if attributes-key strategy proves brittle elsewhere.

### Gap #2 — `EntityKind` variant lost in entity→nodes dual-write
- **Filing:** ISS-120 (proposed) — "Phase B dual-write loses EntityKind variant for entity→nodes; blocks T29.5 entity-reader switch"
- **Severity:** P1 (entity_type is consumed by find_entities/list_entities filters and entity_stats breakdown)
- **Fix option A (preferred):** stamp `kind_label` (output of `kind_to_text`) into `attributes` JSON under reserved key `_legacy_kind`. Patch decoder. Patch backfill.
- **Fix option B:** widen `nodes.node_kind` enum to carry full variant (Person, Concept, Event, Topic, …). Cleanest but schema churn + impacts every code path that filters by `node_kind`.
- **Recommendation:** A.

### Gap #3 — `soft_delete` is single-table (not dual-write)
- **Filing:** ISS-121 (filed) — `soft_delete` updates `memories.deleted_at` only; `nodes.deleted_at` stays NULL forever.
- **Severity:** P1 (every liveness-filter reader diverges the moment any soft-delete happens).
- **Fix:** wrap soft_delete in a transaction, dual-update; backfill driver for existing soft-deleted rows.
- **Recommendation:** ship before any liveness-filtered reader (count_memories_in_namespace, count_soft_deleted, etc.) flips to unified.

### Gap #4 — `nodes_fts` parity not validated
- **Action:** before flipping any FTS reader, run a parity smoke: ingest 50 memories with diverse text (CJK + ASCII), MATCH the same query against both `memories_fts` and `nodes_fts`, assert id sets match. T17 row-count parity test covers row count but not FTS5 token alignment.

### Gap #5 — many aux tables not yet projected
- `pending_memories`, `promotion_*`, `*_quarantine`, `backfill_runs`, `cluster_*` — these are control-plane tables, not subjects of the unified-substrate plan. Defer; readers stay on legacy.

## Recommended Phase D sequencing (revised post-audit)

1. **Land ISS-119 + ISS-120 + ISS-121 dual-write patches** (Gap #1 + #2 + #3). 1-2 days of focused work.
2. **T29.5 entity readers** — once ISS-120 lands, the entity-side readers are mechanical.
3. **T29.6 trivial-safe readers** — `count_*`, `get_namespace`, `list_namespaces`, `get_*_ids` (the ✅ rows above). Can ship as a single sub-task, ~6 readers, ~30 min each. **NOTE:** even the trivial-safe count/list readers depend on ISS-121 (soft_delete dual-write) — without it, liveness filters diverge on any DB that has soft-deletes.
4. **T29.7 hot retrieval readers** — once ISS-119 lands + new decoder helper exists, ship `search_fts`/`fetch_recent`/`fetch_memory_record`/`search_by_type` together. Each gets a parity test (legacy result set vs unified result set on the same DB).
5. **T29.8 embedding readers** — switch the ✅ ones to `node_embeddings`. T20 dual-write already in place.
6. **T30 probe set** — once T29.5/.6/.7 land.
7. **T31 LoCoMo parity** — runs end-to-end against fully-switched Phase D.
8. **T32 flip default** — only after T31 ≥ legacy J-score baseline.

## Followups / open questions

- Topic vs Entity in `count_entities` — current legacy counts `entities` (excludes Topic). Unified `node_kind IN ('entity','topic')` would over-count. Need to decide: keep historical semantics (`node_kind='entity'` only) or widen?
- `get_unenriched_memory_ids` — `enrichment_attempts` is a column on `memories` only. Two options: (a) move to attributes JSON as part of memory→nodes mapping, (b) keep enrichment-tracking on legacy permanently (it's a control-plane concern, not a substrate concern).
- Promotion queue + cluster control-plane tables — likely stay on legacy indefinitely; only "subject" data lives in unified.

---

End of audit.
