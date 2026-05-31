# Phase E Execution Plan — Legacy-Write Deletion (T34–T37)

> engram v0.4 unified-substrate close-out. Goal: disable all legacy dual-writes
> so the legacy tables become unread + unwritten, unblocking T39 DROP.
> **This is a code-deletion job. It does NOT touch any existing data.**

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
  path. (CJK-tokenization diff between paths was already in the read path T31 LoCoMo
  parity validated: legacy 0.3947 vs unified 0.4013, +0.66pp PASS.)
- `nodes_fts_ai` AFTER INSERT trigger auto-populates `nodes_fts` from `nodes`
  inserts → node-side FTS needs no explicit write.
- `insert_memory_node_row` allocates `fts_rowid` from `fts_rowid_counter` → the
  legacy `SELECT rowid FROM memories` lookup is removable wholesale.

---

## 3. SCOPE — exactly 2 files (per design §5.5.1)

- `crates/engramai/src/storage.rs` — 78 legacy writes (memory core, hebbian,
  entities, embeddings, synthesis, FTS)
- `crates/engramai/src/graph/store.rs` — 3 legacy writes (resolution-pipeline edges)

NOT in scope: T38 soak gate (a wait/bench, not code), T39 DROP (human-gated),
T40 VACUUM.

---

## 4. CLUSTER ORDER (lowest-risk first, each = 1 commit + full lib test)

Order is chosen so the *easiest-to-verify* deletions go first, building confidence
before touching the hot recall path or anything decay-related.

- **T34a** — `Storage::add()` legacy `INSERT INTO memories` + FTS-rowid SELECT +
  `INSERT INTO memories_fts`. Survivor: `insert_memory_node_row`. (VERIFIED SAFE)
- **T34b** — `Storage::store_raw()` legacy memories/FTS writes.
- **T34c** — UPDATE family (update/update_content/update_importance) legacy writes.
  (Already dual-write to nodes per ISS-124 — delete legacy half.)
- **T34d** — DELETE family (delete_embedding/hard-delete) legacy writes.
  (ISS-125/126 already dual-DELETE to nodes — delete legacy half.)
- **T35** — Hebbian writes (`hebbian_links`). ⚠️ HIGH RISK: confirm decay/weight
  parity on `edges` BEFORE deleting. Surface to potato at this step, do not barrel.
- **T36** — entity/embedding/synthesis-provenance legacy writes.
- **T37** — `graph/store.rs` 3 resolution-pipeline edge writes (`dual_write_edge_to_edges`).
- **T37x** — EXIT GATE: AST-grep audit proving 0 prod legacy INSERT/UPDATE/DELETE
  remain against legacy tables; full suite green.

## 5. PER-CLUSTER PROTOCOL (non-negotiable)

For EVERY cluster:
1. Read the target lines + verify the unified survivor exists and fires
2. Verify no un-switched read path still reads the legacy table (grep)
3. Verify no test fixture seeds the legacy table directly (else fix fixture FIRST,
   like iss019 — pull nodes row forward, do NOT make child-insert tolerant)
4. Delete the legacy write
5. `cargo test -p engramai --lib` MUST be fully green
6. Commit with cluster id + cite design §5.5.3
7. If ANY non-expected red appears → STOP, file issue, do not force through

**Rule: never delete two clusters without a green test in between.**

## 6. STOP CONDITIONS (when to halt and ask potato)

- T35 Hebbian decay parity unconfirmed
- Any red test that isn't a trivially-explained stale expectation
- Any FK / trigger dependency discovered (ISS-196 was one — expect more)
- Reaching T37x exit gate (report before T38/T39)
- **NEVER touch T39 DROP autonomously** — irreversible, human-gated

---

## 7. STATUS LOG (append as we go)

- 2026-05-31: Plan written. Tag `pre-phase-e-2026-05-31` @ `099362a`. Awaiting
  potato go/no-go on starting T34a.
