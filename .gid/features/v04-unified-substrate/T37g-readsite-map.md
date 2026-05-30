# T37g — `SqliteGraphStore` Read-Site Enumeration Map

> **Status:** enumeration complete (2026-05-30), pre-implementation.
> **Scope:** switch the *retrieval-layer* graph traversal (`SqliteGraphStore`
> in `crates/engramai/src/graph/store.rs`) from reading the legacy
> `graph_entities`/`graph_edges` tables to the unified `nodes`/`edges` tables,
> flag-gated on `unified_substrate`.
> **Prereq:** T37f (done — graph store co-located on substrate file, edge_kind
> remapped to `structural`, ISS-195 resolved, commit `5a5ce76`).
> **Out of scope:** the 1-hop traversal limit (ISS-070) — T37g only switches
> *which substrate* the existing traversal reads from.

---

## 0. Method classification

The 40 `FROM graph_entities`/`FROM graph_edges` grep hits split into three
buckets. Only **bucket A** is T37g scope.

### Bucket A — READ trait methods (T37g SWITCHES these)
The `GraphStore` (read) trait, lines 227–424. Every method below is consumed
by `AssociativePlan` / resolution / KC retrieval and must read unified tables
when `unified_substrate=true`:

| # | method | line | reads | notes |
|---|--------|------|-------|-------|
| A1 | `get_entity` | 1864 | graph_entities | single-row by id |
| A2 | `list_entities_by_kind` | 1971 | graph_entities (id only) | filter by kind+ns, order by recency |
| A3 | `search_candidates` | 2010 | graph_entities ×4 (2131/2137/2191/2198) | alias/name/embedding candidate gen |
| A4 | `get_edge` | 2373 | graph_edges | single-row by id |
| A5 | `find_edges` | 2400 | graph_edges ×6 (2443/2459/2499/2517/2556/2575) | predicate/subject/object filters |
| A6 | `edges_of` | 2609 | graph_edges ×4 (2640/2665/2694/2718) | outgoing edges of subject |
| A7 | `edges_into` | 2735 | graph_edges ×4 (2765/2790/2819/2843) | incoming edges to object |
| A8 | `edges_as_of` | 2860 | graph_edges (2905) | temporal-valid edges at instant |
| A9 | `traverse` | 2937 | graph_edges ×4 (3050/3091/3192/3246) | BFS multi-hop — the hot path |
| A10 | `edges_in_episode` | 3347 | graph_edges (3353) | edges by episode_id |
| A11 | `entities_linked_to_memory` | 3426 | (resolves via edges) | memory → entity ids |
| A12 | `memories_mentioning_entity` | 3454 | (resolves via edges) | entity → memory ids |
| A13 | `edges_sourced_from_memory` | 3482 | graph_edges (3495) | edges by source_memory |
| A14 | `list_namespaces` | 3909 | graph_entities (3915) | DISTINCT namespace |

### Bucket B — WRITE methods (NOT T37g; dual-write already lands rows in nodes/edges)
`insert_entity` (3928), `update_entity_cognitive` (4010),
`touch_entity_last_seen` (4062), `update_entity_embedding` (4091),
`upsert_alias` (4142), `merge_entities` (4181, reads graph_entities at
4233/4253 + graph_edges at 4349 *as part of the write txn*), `insert_edge`
(4494), `invalidate_edge` (4598, reads 4618/4650), `supersede_edge` (4678,
reads 4785), `link_memory_to_entities` (4825), topic writers, pipeline-run
writers, `apply_graph_delta` (5365, reads graph_entities at 5449/5459 inside
the delta-apply txn).

These keep writing to legacy tables (dual-write to nodes/edges already exists
from T13–T16). Their *internal reads* are read-modify-write on the legacy
authoritative copy and stay legacy until Phase E deletes the legacy writes
(T34–T37). **Switching their reads now would split the read/write substrate
mid-transaction — explicitly out of T37g scope.**

### Bucket C — non-SQL / doc / helper hits
Lines 1714, 1744 (doc comments), 3508/3517 (`get_topic*` — reads
`knowledge_topics`, not graph_entities/graph_edges; unaffected).

---

## 1. Field-translation reference (legacy → unified)

### 1.1 Entity: `graph_entities` → `nodes WHERE node_kind IN ('entity','topic')`

| legacy column | unified column | translation |
|---|---|---|
| `id` (16-byte UUID BLOB) | `id` (36-char TEXT) | `hyphenated-lowercase UUID string`; the dual-write key is `entity.id.to_string()` |
| `canonical_name` | `content` | direct (entity canonical name stored as node content) |
| `kind` (EntityKind text) | `node_kind` + `attributes._legacy_kind` | `Topic→'topic'`, else `'entity'`; full EntityKind serde-stamped under `attributes._legacy_kind` (ISS-120) — reader reconstructs `entity_type` from there |
| `summary` | `summary` | direct |
| `attributes` (JSON) | `attributes` (JSON, minus `_legacy_kind`) | reader must STRIP reserved `_legacy_kind` before returning user attributes (history is a separate column, not in attributes) |
| `first_seen`/`last_seen` | `first_seen`/`last_seen` | direct (REAL unix) |
| `created_at`/`updated_at` | `created_at`/`updated_at` | direct |
| `activation`/`arousal`/`importance` | same | direct |
| `agent_affect` (JSON) | `agent_affect` | direct |
| `identity_confidence` | `confidence` | direct |
| `somatic_fingerprint` (BLOB) | `somatic_fingerprint` | direct |
| `history` (JSON) | `history` (TEXT, real column) | **direct** — `nodes.history` exists (storage.rs:654), dual-write copies verbatim |
| `merged_into` (BLOB) | — (**NO unified home**) | **RISK R1: `dual_write_entity_to_nodes` does NOT write `merged_into`.** `nodes.superseded_by` exists but is unused by the entity dual-write. A merged-away entity read via unified would lose its `merged_into` pointer → `resolve_alias`/`merge_entities` follow-the-pointer logic breaks. See §3. |
| `embedding` (BLOB) | `embedding` (BLOB) | direct |

### 1.2 Edge: `graph_edges` → `edges WHERE edge_kind='structural'`

| legacy column | unified column | translation |
|---|---|---|
| `id` (BLOB) | `id` (TEXT) | hyphenated UUID string |
| `subject_id` (BLOB) | `source_id` (TEXT) | hyphenated UUID string |
| `predicate_kind` | `predicate_kind` | direct |
| `predicate_label` | `predicate` | direct (label → the single `predicate` string) |
| `object_kind` | — (derived) | NOT stored; reader derives: `target_id NOT NULL → Entity`, `target_literal NOT NULL → Literal` |
| `object_entity_id` (BLOB) | `target_id` (TEXT) | hyphenated UUID string |
| `object_literal` | `target_literal` | direct (JSON text) |
| `summary` | `summary` | direct |
| `valid_from`/`valid_to` | same | direct |
| `recorded_at` | `recorded_at` | direct |
| `invalidated_at`/`invalidated_by` | same | direct (invalidated_by → TEXT uuid) |
| `supersedes` | `supersedes` | direct (TEXT uuid) |
| `episode_id` | — (DROPPED) | design §7.4; A10 `edges_in_episode` has NO unified equivalent — see §3 |
| `memory_id` | `source_memory_id` | **Phase-B NULL** — see §3 risk |
| `resolution_method` | `resolution_method` | direct |
| `activation`/`confidence` | same | direct |
| `agent_affect` | `agent_affect` | direct |
| `created_at` | `created_at` | direct |

### 1.3 Filter clause translations

- legacy `WHERE id = ?1` with `id.as_bytes().to_vec()` BLOB param
  → unified `WHERE id = ?1` with `id.to_string()` TEXT param.
- legacy `WHERE subject_id = ?1 AND object_entity_id = ?6` (BLOB params)
  → unified `WHERE source_id = ?1 AND target_id = ?6` (TEXT params).
- legacy `WHERE predicate_kind = ?3 AND predicate_label = ?4`
  → unified `WHERE predicate_kind = ?3 AND predicate = ?4`.
- legacy entity scan `FROM graph_entities WHERE namespace = ?`
  → unified `FROM nodes WHERE node_kind IN ('entity','topic') AND namespace = ?`
  (MUST add the `node_kind` filter — `nodes` also holds memories/insights).
- legacy edge scan `FROM graph_edges`
  → unified `FROM edges WHERE edge_kind = 'structural'`
  (MUST add `edge_kind` filter — `edges` also holds associative/containment/provenance).

---

## 2. Flag threading (prerequisite step T37g-0)

`grep -rn unified_substrate crates/engramai/src/graph/` → **0 hits today**.
`SqliteGraphStore` has no flag. Before any read-switch:

- Add `unified_substrate: bool` to `SqliteGraphStore<'a>` (default `false`).
- Thread it from `MemoryConfig.unified_substrate` at the construction sites:
  `with_graph_store` (memory.rs:505) and `with_pipeline_pool` (memory.rs:314).
- Mirror the T29.5 storage-layer pattern: each switched reader branches
  `if self.unified_substrate { /* unified SQL */ } else { /* legacy SQL */ }`.
- Default-off preserves byte-identity (§5.4 envelope) until benched.

## 3. Risks & open items (must resolve before/within implementation)

- **R1 — `merged_into` has no unified home.** The entity dual-write drops the
  merge pointer. Methods that follow it (`resolve_alias`, `get_entity` returning
  a tombstoned/merged entity, `merge_entities` internal reads) cannot be
  switched correctly until either (a) dual-write stamps `merged_into` into
  `nodes.superseded_by` or an `attributes._merged_into` key, or (b) T37g adds a
  backfill. **Recommendation:** extend `dual_write_entity_to_nodes` to write
  `merged_into → superseded_by` (semantically exact: a merged entity IS
  superseded by its survivor) as a small pre-T37g fix, mirror in nodes UPSERT.
  **Empirical (conv-26 parity DB 2026-05-30):** `merged_into` non-null = **0**,
  `nodes.superseded_by` non-null = **0** → latent, does NOT bite conv-26, but
  must be fixed before a merge-heavy corpus or before T39 DROP.
- **R2 — unified entity-node count gap.** **Empirical (conv-26 parity DB):**
  `graph_entities` = **674**, `nodes WHERE node_kind='entity'` = **668** → a
  **6-entity** dual-write gap. There are **0** short-hex (non-16-byte) ids on
  this corpus (the earlier "3 legacy short-hex" was a stale-snapshot artifact —
  superseded by this measurement). The 6 missing entity nodes must be root-caused
  (dedup-merge collapsing? topic vs entity miscount? — note 674 includes topics)
  before switching readers, OR accepted as a quantified gap with a backfill.
  **Action:** re-run the count on next parity DB and diff the missing 6 ids.
- **R3 — `edges_in_episode` (A10) has no unified equivalent.** `episode_id` is
  dropped from `edges` (design §7.4). Either (a) keep A10 legacy-only with a
  doc note (it is used by KC/pipeline introspection, not the hot AssociativePlan
  path), or (b) route episode scoping through containment edges. **Recommendation:**
  keep legacy-only for T37g, file follow-up; it is not on the retrieval hot path.
- **R4 — `source_memory_id` is Phase-B NULL in unified edges.** Any reader that
  filters/returns the source memory (`edges_sourced_from_memory` A13,
  `memories_mentioning_entity` A12, `entities_linked_to_memory` A11) cannot get
  the memory id from unified `edges.source_memory_id` (always NULL today). These
  three methods depend on the legacy `graph_edges.memory_id` column.
  **Recommendation:** A11/A12/A13 stay legacy-only in T37g — they are blocked on
  T12 memory-id backfill into `edges.source_memory_id`. Document as T37g-deferred.
- **R5 — `object_kind` is derived, not stored.** Reader must reconstruct
  `EdgeEnd::Entity` vs `EdgeEnd::Literal` from `(target_id, target_literal)`
  nullness, matching the CHECK invariant. Trivial but must be in `decode_edge_row`.

## 4. Implementation order (simplest → hottest)

Each step: add unified branch behind flag, add a parity test asserting
unified-on == legacy-off on a fixture, run `cargo test -p engramai --lib`.

1. **T37g-0** thread `unified_substrate` into `SqliteGraphStore` (+ R1 merge fix).
2. **A1 `get_entity`** — single row, cleanest mapping, exercises entity decode.
3. **A4 `get_edge`** — single row, exercises edge decode + R5 object_kind derive.
4. **A2 `list_entities_by_kind`** — id-only projection + node_kind filter.
5. **A14 `list_namespaces`** — DISTINCT namespace (entity scan).
6. **A3 `search_candidates`** — 4 sub-queries (alias/name/embedding).
7. **A5 `find_edges`** — 6 sub-queries, triple + slot lookup.
8. **A6 `edges_of`** / **A7 `edges_into`** — outgoing/incoming neighborhoods.
9. **A8 `edges_as_of`** — temporal-valid filter.
10. **A9 `traverse`** — the multi-hop BFS hot path (LAST — highest blast radius).
11. **DEFERRED (stay legacy, doc note):** A10 `edges_in_episode` (R3),
    A11/A12/A13 memory-id readers (R4). File follow-up issue.

## 5. Acceptance

- Flag-off: byte-identical to today (parity tests + §5.4 envelope).
- Flag-on: A1–A9 read from unified `nodes`/`edges`; conv-26 LoCoMo multi-hop
  parity (within §5.4 LLM-judge wobble) vs flag-off.
- After T37g + R4 unblock, T39 can DROP `graph_entities`/`graph_edges` without
  breaking retrieval.
