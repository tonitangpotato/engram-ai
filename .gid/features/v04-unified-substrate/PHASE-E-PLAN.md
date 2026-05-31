# Phase E Execution Plan — Legacy-Write Deletion (T34–T37) — r2

> engram v0.4 unified-substrate close-out. Goal: disable all legacy dual-writes
> so the legacy tables become unread + unwritten, unblocking T39 DROP.
> **This is a code-deletion job. It does NOT touch any existing data.**
>
> r2 (2026-05-31): revised per review `reviews/phase-e-plan-r1.md`. All write
> counts now reflect VERIFIED prod-region (line < test-mod-boundary) ground truth.

---

## 0. WHY (the one-sentence reason)

engram's thesis is **"the graph IS the substrate"** — there cannot be two DBs
(potato: 不可能有两套DB的). Historically memories lived in `memories`/`memories_fts`
and the semantic graph lived in `graph_entities`/`graph_edges` (a *separate file*
graph.db in separate-file mode). v0.4 unifies both into `nodes` + `edges` in a
single `substrate.db`.

- Write-path unification: DONE (ISS-195 — substrate.db holds 694 entity / 783 edge)
- Read-path unification: DONE (T37g — all reads hit nodes/edges, parity 12/12)
- **Current state: DUAL-WRITE** — every write hits BOTH new (`nodes`) and old
  (`memories`) tables. This is the migration safety-net.

**Phase E removes the legacy INSERT statements.** After Phase E the legacy tables
are written by nobody and read by nobody → T39 can safely DROP them → "one table,
one DB" is physically achieved.

---

## 1. ROLLBACK SAFETY (three layers)

| Layer | Mechanism | Recovery command |
|---|---|---|
| Tag checkpoint | git tag before any deletion | `git reset --hard pre-phase-e-2026-05-31` |
| Per-cluster commit | each cluster = 1 isolated commit | `git revert <cluster-sha>` |
| **Data immunity** | Phase E deletes INSERT *code*, never DROPs *data* | legacy tables keep all rows until T39 (separate, human-gated) |

**Key insight:** Phase E is non-destructive to data. Even total botch = zero data
loss, because the legacy `memories`/`graph_edges` rows stay until T39. T39 (the
only irreversible step) stays in potato's hands.

Checkpoint tag: `pre-phase-e-2026-05-31` @ commit `099362a`

---

## 2. PRE-FLIGHT VERIFIED FACTS (ground truth, not assumption)

- ISS-196 FK blocker resolved: `access_log.memory_id` now REFERENCES `nodes(id)`,
  `add()`/`store_raw` reordered (node row first). Commit `f38175c`.
- t13/t17 stale-test drift fixed (assertion→structural per T37f). Commit `099362a`.
- Full suite GREEN: **2441 pass / 0 fail**.
- FTS read-switch (T29.6) gates reads on `unified_substrate` flag → under flag,
  reads hit `nodes_fts` exclusively. Legacy `memories_fts` write feeds a now-unread
  path. (CJK-tokenization diff was already in the read path T31 LoCoMo parity
  validated: legacy 0.3947 vs unified 0.4013, +0.66pp PASS.)
- `nodes_fts_ai` AFTER INSERT trigger auto-populates `nodes_fts` from `nodes`
  inserts → node-side FTS needs no explicit write.
- **Test-module boundaries** (writes BELOW these lines are test code, NOT prod
  deletion targets): storage.rs `#[cfg(test)]` @ L8741; graph/store.rs @ L6887.

---

## 3. SCOPE — verified prod-region write inventory (2 files)

### 3.0 Per-table prod write counts (line < test boundary)

`crates/engramai/src/storage.rs` (prod region, < L8741):

- `memories` INSERT: **3**
- `memories_fts` INSERT: **7**
- `hebbian_links` INSERT: **5**
- `memory_entities` INSERT: **2**
- `synthesis_provenance` INSERT: **1**
- `memory_embeddings` INSERT: **1**
- `memory_embeddings_v2` INSERT: **1**
- `triples` INSERT: **1**
- `UPDATE memories`: **14**
- `DELETE FROM memories`: **2**
- `DELETE FROM memories_fts`: **6**
- `DELETE FROM hebbian_links`: **6**
- `DELETE FROM memory_entities`: **3**
- `DELETE FROM synthesis_provenance`: **2**
- `DELETE FROM memory_embeddings`: **3**

storage.rs prod total: **57** legacy write statements.

`crates/engramai/src/graph/store.rs` (prod region, < L6887):

- `graph_entities`/`graph_edges` INSERT: **6** (insert_entity@4874, merge_entities@5338,
  insert_edge@5439, supersede_edge@5650, apply_graph_delta@6450+6577)
- KEEP (unified survivors, do NOT delete): `dual_write_edge_to_edges` /
  `dual_write_entity_to_nodes` call sites — these write the NEW edges/nodes.

graph/store.rs prod total: **6** legacy write statements.

**GRAND TOTAL prod legacy writes to remove: 63.** (The design's "81" / r1's "78"
were stale inventory numbers conflating test code + DDL + survivor calls.)

### 3.1 Legacy-write → unified-survivor map (deletion is safe ONLY when survivor verified)

A legacy write may be deleted ONLY if a unified-side counterpart already exists and
is verified to fire. Any UNVERIFIED row blocks its cluster until confirmed.

| Legacy write | Unified survivor | Verified? |
|---|---|---|
| `INSERT INTO memories` (add) | `insert_memory_node_row` | ✅ (this session) |
| `INSERT INTO memories_fts` | `nodes_fts_ai` trigger on nodes | ✅ (T29.6 + trigger) |
| `INSERT INTO memories` (store_raw) | nodes insight INSERT OR IGNORE | ✅ (ISS-196 reorder) |
| `UPDATE memories` (update*) | ISS-124 dual-update to nodes | ⬜ confirm each of 14 |
| `UPDATE memories superseded_by` | T12 dual-UPDATE memories+nodes | ⬜ confirm |
| `DELETE FROM memories*` | ISS-126 hard-delete cascade nodes | ⬜ confirm each |
| `DELETE FROM memories_fts` | `nodes_fts_ad` trigger on nodes | ⬜ confirm |
| `INSERT INTO hebbian_links` | `record_coactivation` → edges(associative) | ⬜ confirm (T35) |
| `DELETE FROM hebbian_links` | edges associative delete path | ⬜ confirm (T35) |
| `INSERT INTO memory_entities` | ISS-123 link_memory_entity → edges | ⬜ confirm |
| `INSERT INTO synthesis_provenance` | T29.2 provenance → edges | ⬜ confirm |
| `INSERT INTO memory_embeddings(_v2)` | node_embeddings table | ⬜ confirm |
| `INSERT INTO triples` | **NONE — table is drop-set, 0 readers** | ✅ delete outright (see FINDING-3 / T36b) |
| `INSERT INTO graph_entities/edges` (×6) | dual_write_entity_to_nodes / dual_write_edge_to_edges | ⬜ confirm each of 6 |

Per-cluster step-2 (below) fills the ⬜ rows by reading the survivor before deleting.

---

## 4. CLUSTER ORDER (lowest-risk first, each = 1 commit + full lib test)

Each cluster lists the prod-region statements it owns. Clusters are
**organizational, not strict arithmetic partitions** — line numbers shift as
deletions happen, so the authoritative completeness check is the §6.1 grand-total
grep (63 → 0), NOT a fixed per-cluster line map.

- **T34-pre** — Phase B contract-test migration (NO prod code change). The
  v04_phase_b_dual_write.rs t12 suite asserts dual-write (memories row == nodes row).
  Those assertions' real value is *field-completeness* (no silent field loss), not
  *two-table-equality*. Migrate 10 `FROM memories` assertions across the suite to
  read from `nodes` instead, while dual-write still exists (so they pass immediately).
  This decouples "rewrite tests" from "delete prod", making T34a a pure deletion.
  Decision: option (b) from review — preserve field-completeness regression value,
  narrow assertion target from memories→nodes. Sites: L94/L118 (scalar equality),
  L268/290/323/372 (superseded_by), L1300/1779/1787/1949 (count/content).
- **T34a** — `Storage::add()`: memories INSERT + FTS-rowid SELECT + memories_fts
  INSERT. Survivor: insert_memory_node_row. (3 stmts; VERIFIED SAFE; runs AFTER T34-pre)
- **T34b** — `Storage::store_raw()`: legacy memories/FTS writes.
- **T34c** — UPDATE family: all 14 `UPDATE memories` + superseded_by paths.
  Confirm each has ISS-124/T12 dual-update survivor.
- **T34d** — DELETE family: 2 memories + 6 memories_fts deletes.
  Confirm ISS-126 cascade + nodes_fts_ad trigger survivors.
- **T35** — Hebbian: 5 INSERT + 6 DELETE on hebbian_links. ⚠️ HIGH RISK: confirm
  decay/weight/coactivation parity on edges(associative) BEFORE deleting.
  **Surface to potato at this step, do not barrel.**
- **T36a** — entity (2) + embedding (1+1) + synthesis_provenance (1) INSERT +
  their DELETEs (3+3+2). Confirm ISS-123 / T29.2 / node_embeddings survivors.
  ⚠️ STOP-CONDITION: `memory_embeddings_v2` has NO design hit confirming
  `node_embeddings` is its unified survivor — step-1 MUST prove node_embeddings is
  both written by a unified path AND read by unified retrieval BEFORE deleting
  either embedding write, else embeddings are lost silently.
- **T36b** — `triples` INSERT (storage.rs:7296). NO survivor needed — table is in
  drop-set (design §7.6, 0 rows, no reader). Delete outright. ⚠️ Check entanglement
  with T26a's noted triple-path dual-write debt (store_triples entity writes) BEFORE
  deleting; if entangled, defer with tracking ref.
- **T37** — graph/store.rs: 6 prod legacy INSERT (insert_entity / merge_entities /
  insert_edge / supersede_edge / apply_graph_delta ×2). KEEP the 3
  dual_write_*_to_* survivor calls.
- **T37x** — EXIT GATE: see §6.1. Full suite green + 0 remaining prod legacy writes.

## 5. PER-CLUSTER PROTOCOL (non-negotiable)

For EVERY cluster:
1. Read the target lines + verify the unified survivor exists and fires (fill the
   §3.1 ⬜ rows for this cluster's writes)
2. Verify no un-switched read path still reads the legacy table (grep)
3. Verify no test fixture seeds the legacy table directly (else fix fixture FIRST,
   like iss019 — pull nodes row forward, do NOT make child-insert tolerant)
4. Delete the legacy write
5. `cargo test -p engramai --lib` MUST be fully green
6. Commit with cluster id + cite design §5.5.3
7. If ANY non-expected red appears → STOP, file issue, do not force through

**Rule: never delete two clusters without a green test in between.**
**Rule: a write with an UNVERIFIED survivor (⬜) is NOT deleted — confirm first.**

## 6. STOP CONDITIONS (when to halt and ask potato)

- T35 Hebbian decay parity unconfirmed
- T36b triples-path entanglement with T26a dual-write debt
- T36a `memory_embeddings_v2` survivor (`node_embeddings`) unconfirmed in design
- Any §3.1 survivor that turns out NOT to exist (would be silent data-write loss)
- Any red test that isn't a trivially-explained stale expectation
- Any FK / trigger dependency discovered (ISS-196 was one — expect more)
- Reaching T37x exit gate (report before T38/T39)
- **NEVER touch T39 DROP autonomously** — irreversible, human-gated

### 6.1 T37x exit-gate method (explicit)

Run, over `src/` EXCLUDING test modules (line < boundary) and migration DDL:

```
grep -nE "(INSERT( OR (IGNORE|REPLACE))?|UPDATE|DELETE FROM) +(memories|memories_fts|hebbian_links|memory_entities|synthesis_provenance|memory_embeddings|memory_embeddings_v2|graph_entities|graph_edges|triples)\b"
```

Expect **0 matches** in prod regions (excluding migration `CREATE`/DDL and the
retained `access_log`). Cross-check the count drops from 63 → 0 cluster-by-cluster.

---

## 8. PHASE E-0 — READ-CUTOVER PREREQUISITE (ISS-197) ⚠️ MUST PRECEDE T34a

### 8.0 Why this section exists

T34a attempted as a pure deletion (2026-05-31) → **104 lib test failures**.
Root cause (ISS-197): removing the legacy `memories` WRITE orphans the prod
`SELECT FROM memories` READ sites that T29.7 deferred to "Phase F prep". Those
reads must be cut over to `nodes` FIRST. The cutover is **independently
shippable under the live dual-write** (reads from `nodes` return byte-identical
rows because `nodes` is dual-written today). Only after all reads pass on
`nodes` does T34a become a genuine pure deletion.

**Decoder is already written**: `row_to_record_from_node_impl`
(`storage.rs` ~L8651, currently `#[allow(dead_code)]`) decodes a `nodes`
row → full `MemoryRecord` (reads `attributes` not `metadata`; extracts
`_legacy_contradicts`/`_legacy_contradicted_by` from attributes JSON).
`nodes` schema (L608) carries every column the decoder needs.

### 8.1 NOT in scope (do NOT touch)

- **FTS-write companion reads** — `SELECT rowid FROM memories WHERE id=?` at
  L1968, L2778, L2874, L2945, L4417, L6820. These fetch the legacy `memories`
  rowid solely to drive a `memories_fts` / `UPDATE memories` write. They
  vanish *with* their paired write-deletion (T34a/T34c/T34d), NOT via read
  cutover.
- **Migration-source reads** — `substrate/backfill.rs` (9),
  `substrate/verify.rs` (1), `substrate/triple_backfill.rs` (1). These read
  `memories` *as the source of truth to migrate FROM*; they must keep reading
  `memories` until T39 DROP.
- **`rebuild_fts` legacy** (L1868 COUNT, L1882 rowid,content) — operates on the
  legacy `memories_fts` index; retired with the FTS write, not cut over.

### 8.2 IN scope — read-cutover buckets

**Bucket A — `SELECT *` + decoder swap** (decode `nodes` via
`row_to_record_from_node_impl`; rewrite `FROM memories` →
`FROM nodes WHERE node_kind='memory'`, remap column-specific WHERE clauses):
L2710, L2733, L2981, L2993, L3057(m.*), L3068(m.*), L3167, L3177, L3191,
L3840, L3852, L3993, L4458, L4467, L8480, L8503(`*, namespace`),
L8388/L8397 (`id, content, metadata` projections).

**Bucket B — scalar single-column reads** (rewrite source table + column;
`metadata`→`attributes` where applicable): L3657(namespace), L4491(deleted_at),
L6180(content), L6218(content,metadata→attributes), L7019(created_at),
L7118(metadata→attributes), L7275(id WHERE created_at/namespace),
L7534(id), L3014(namespace) + lifecycle.rs L143/L171(COUNT) + L315(metadata)
+ graph/store.rs L6815(entity_ids,edge_ids — verify these live in nodes
attributes vs T37c denormalised columns).

**Bucket C — subquery liveness/dangling refs** (rewrite the
`SELECT id FROM memories WHERE deleted_at IS NULL` truth-source subqueries):
L1661, L1663, L1683, L1685 (namespace subqueries in edge insert), L7962,
L7970, L7978 (rebalance dangling cleanup `NOT IN (SELECT id FROM memories)`),
L1315/L1325 (soft-delete count/deleted_at), L1382/L1392 (id scans).

**Bucket D — half-cut / anomalous** (inspect individually before touching):
L3121 `search_fts` arm already joins `nodes_fts` via `n.fts_rowid` but still
says `FROM memories m` with alias mismatch — likely an incomplete T29.6 cut;
resolve to clean `FROM nodes n`. L3132 is the legacy `memories_fts` fallback
arm (flag-gated) — confirm it retires with FTS write, not cut over.

### 8.3 ⚠️ SEMANTIC GOTCHA — `superseded_by` NULL vs `''`

Legacy `memories.superseded_by` uses **`''`** (empty string) for "not
superseded". Unified `nodes.superseded_by` is `TEXT REFERENCES nodes(id)` and
uses **`NULL`** (never `''`; insert path always writes NULL, design L2020).
When rewriting predicates:
- "not superseded" filter `(superseded_by IS NULL OR superseded_by = '')`
  works on `nodes` as-is (the `= ''` arm is harmlessly never-true).
- "is superseded" filter `superseded_by != ''` MUST become
  `superseded_by IS NOT NULL` (SQL `NULL != ''` is NULL/falsy, so a literal
  swap is fragile — use the IS NOT NULL form). Applied in `list_superseded`.

### 8.3b Per-site protocol (one site or one cohesive method at a time)1. Rewrite SELECT source + decoder + WHERE column remap.
2. `cargo test -p engramai --lib` MUST stay green (2075+). Because dual-write
   is live, `nodes` and `memories` hold identical rows → reads are byte-identical.
3. Commit per bucket (or per method group) citing ISS-197 §8.2 + bucket id.
4. After ALL buckets green → re-attempt T34a as pure deletion (ISS-197 AC-3).

### 8.4 Exit gate (E-0 → T34a)

`awk 'NR<TESTBOUNDARY' src | grep 'FROM memories'` over storage.rs +
lifecycle.rs + graph/store.rs returns ONLY the §8.1 not-in-scope sites
(FTS-companion + rebuild_fts). Migration-source files (backfill/verify/
triple_backfill) excluded. Then T34a/T34c/T34d delete writes AND their
paired FTS-companion reads together.

### 8.5 ⚠️ BLOCKER (discovered during E-0) — hebbian-dedup namespace lookup stays on memories

`merge_bidirectional_hebbian` (~L1661-1685) reads `m.namespace` to compute the
canonical `(ns, id)` tuple ordering for dedup. It was cut to `nodes` and caused
**212 test failures** with `no such table: nodes`, because this dedup SQL runs
**unconditionally during migration** (no `unified_substrate` guard) at a point
*before* `migrate_unified_nodes` is guaranteed to have created `nodes` — and on
separate-file graph DB connections that never have `nodes`. Reverted. The lookup
is advisory (COALESCE `''` fallback on missing rows), so it stays on `memories`
until the table is dropped at T39. **Rule learned:** read-cutover only applies to
reads behind a `unified_substrate` guard OR app-level `Storage`-method reads
(Storage always has `nodes` via unconditional `migrate_unified_nodes`). Migration/
maintenance SQL that runs during construction is OUT of scope.

### 8.6 ⚠️ BLOCKER (discovered during E-0) — `get_deleted_at` column-type mismatch

`memories.deleted_at` is **TEXT (RFC3339)** (L488 `ALTER TABLE memories ADD COLUMN
deleted_at TEXT`) but `nodes.deleted_at` is **REAL (epoch f64)** (L649 schema).
`soft_delete` dual-writes BOTH: `now_rfc` to memories, `now_epoch` to nodes (an
existing, deliberate divergence — see comment at L8769 "reads deleted_at as REAL
… not currently surfaced"). `get_deleted_at` returns `Option<String>`; reading the
REAL nodes column as String errors in rusqlite (caught by `test_forget_targeted_soft`).
Cutting it over requires changing the return type or an epoch→RFC3339 conversion —
out of mechanical-cutover scope. Stays on `memories`. **The deleted_at type/format
must be reconciled as a T39 prerequisite** (either make nodes store TEXT, or make
all readers epoch-native).

---

## 7. STATUS LOG (append as we go)

- 2026-05-31 r1: Plan written. Tag `pre-phase-e-2026-05-31` @ `099362a`.
- 2026-05-31 r2: Revised per review. Verified prod counts: storage.rs 57 +
  graph/store.rs 6 = **63** legacy writes. Added §3.1 survivor map, T36b for triples,
  §6.1 explicit exit-gate.
- 2026-05-31 r2-final: Applied review r2 (FINDING-7 clusters are organizational not
  arithmetic partitions; FINDING-8 memory_embeddings_v2 stop-condition for T36a).
  Plan is execution-ready. Awaiting potato go/no-go on T34a.
- 2026-05-31 ISS-197: T34a attempted → 104 failures → reverted clean (suite 2075/0).
  Discovered Phase E-0 read-cutover prerequisite. Added §8 with 66-site bucketed
  inventory (Buckets A/B/C/D + not-in-scope FTS-companion/migration-source).
  Confirmed decoder `row_to_record_from_node_impl` already exists. potato approved
  Phase E-0. Starting Bucket A (lowest risk, decoder swap under live dual-write).
- 2026-05-31 E-0 COMPLETE (read-cutover cohorts all green 2075/0). Commits:
  - Bucket A: 99bfaae (all/get_by_ids) · 1cd72be (search_by_type/get_recent) ·
    6e7bb2e (list_superseded/all_in_namespace/list_deleted) · 149412f (fetch
    helpers + insight-predicate + merge_enriched_into nodes-mirror).
  - Bucket D: cd12c66 (search_fts + search_fts_ns both arms → pure `FROM nodes n`,
    dropped the `memories m JOIN` left over from incomplete T29.6).
  - Bucket B: c94411c (get_namespace/get_memory_content_preview/get_memory_timestamp/
    get_memory_ids_since) · 98ac819 (count_memories/list_namespaces/count_orphan_memories/
    get_orphan_memory_ids/count_dangling_hebbian/count_stale_clusters/
    count_memories_in_namespace + cleanup_orphaned_access_log/cleanup_dangling_hebbian/
    cleanup_orphaned_entity_links) · 50f8157 (get_memories_without_embeddings unified
    arm/embedding_stats total/get_memories_without_entities) · d4d3634
    (count_soft_deleted + get_deleted_at blocker doc).
  - **Predicate:** all scans use `node_kind IN ('memory','insight')` (legacy
    `memories` held synthesis insights too — store_raw writes node_kind='insight').
    id-keyed point reads need no kind filter.
  - **Two NEW blockers found + documented (NOT forced):**
    - §8.5: hebbian-dedup namespace subqueries (merge_bidirectional_hebbian
      ~L1661-1685) run UNCONDITIONALLY during migration, before `nodes` is
      guaranteed to exist (separate-file graph DB + migration ordering) → 212
      `no such table: nodes` failures → reverted, stays on memories (advisory
      COALESCE '' fallback, retires at T39).
    - §8.6: `get_deleted_at` type mismatch — `memories.deleted_at` is TEXT/RFC3339
      (L488 ALTER) but `nodes.deleted_at` is REAL/epoch (L649 schema); soft_delete
      dual-writes BOTH formats. `get_deleted_at` returns Option<String>; reading
      the REAL nodes column as String errors (test_forget_targeted_soft). Left on
      memories — type/return reconciliation is a T39 prerequisite.
  - **EXIT GATE PASSED:** residual `FROM memories` reads are ALL §8.1 not-in-scope:
    FTS-companion rowid lookups, legacy `else`-arms of cut unified reads, RMW-paired-
    with-write (6276 dedup content-evolution, 7216 append_merge_provenance — these
    need a nodes-mirror like merge_enriched_into got, OR retire with their write),
    enrichment-status (7637 triple_extraction_attempts), v1 migration-source
    (8508/8517), test code. Ready to RE-ATTEMPT T34a.
  - **T34a caution:** soft_delete + 6276 + 7216 still WRITE memories. T34a must
    delete ONLY the `INSERT INTO memories` in Storage::add — NOT these UPDATE paths
    (they retire later / need nodes-mirror first).

### §8.7 — T34a RE-ATTEMPT BLOCKED: retained child-table FKs still target `memories` (2026-05-31)

T34a was re-attempted with the correct narrow scope (gate `INSERT INTO memories`
+ `memories_fts` in `Storage::add` behind `if !unified`, keep
`insert_memory_node_row`, leave RMW UPDATE paths + soft_delete writing memories).
Result: **2052 passed / 23 failed**, all uniform `FOREIGN KEY constraint failed`.

**Root cause (NOT a read-cutover gap):** ISS-196 re-pointed ONLY
`access_log.memory_id` → `nodes(id)`. Three other RETAINED child tables, written
during `add`/enrichment, still declare FKs into `memories(id)`:

- `hebbian_links` (L1087-1088): `source_id`/`target_id` → `memories(id)`, written
  by `record_coactivation*` during `add` co-activation → **the FK firing** for the
  lifecycle/broadcast failures.
- `memory_entities` (L1139): `memory_id` → `memories(id)`, written during entity
  enrichment.
- `synthesis_provenance` (L1168-1169): `insight_id`/`source_id` → `memories(id)`,
  written when synthesis insights land.

With `PRAGMA foreign_keys=ON` and `memories` no longer populated under unified,
the legacy FK fires on the child insert before the unified `edges`/`nodes`
dual-write matters. These tables DO dual-write to unified, but the legacy child
table still exists with its old FK.

**True T34a prerequisite (T34a-pre):** re-point ALL retained child-table FKs from
`memories(id)` to `nodes(id)` — the exact `migrate_access_log_fk_to_nodes`
table-rebuild pattern, applied to `hebbian_links`, `memory_entities`,
`synthesis_provenance`. Bounded + mechanical (3 idempotent rebuilds), but NOT
inside the T34a deletion commit (conflates concerns).

**Stop-condition honoured:** did NOT rewrite the 23 tests or disable FK
enforcement. Reverted uncommitted T34a edit (`git checkout storage.rs`), tree
clean at `225cd3a`, suite green 2075/0. No commits, no data touched.

**Open question (potato):** widen ISS-196 (it already owns the access_log
re-point, same rationale) OR new T34a-pre sub-task here in §8?
