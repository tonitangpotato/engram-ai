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

## 7. STATUS LOG (append as we go)

- 2026-05-31 r1: Plan written. Tag `pre-phase-e-2026-05-31` @ `099362a`.
- 2026-05-31 r2: Revised per review. Verified prod counts: storage.rs 57 +
  graph/store.rs 6 = **63** legacy writes. Added §3.1 survivor map, T36b for triples,
  §6.1 explicit exit-gate.
- 2026-05-31 r2-final: Applied review r2 (FINDING-7 clusters are organizational not
  arithmetic partitions; FINDING-8 memory_embeddings_v2 stop-condition for T36a).
  Plan is execution-ready. Awaiting potato go/no-go on T34a.
