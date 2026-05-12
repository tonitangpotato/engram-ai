# Design Review r2 Part 1 — Foundation (§0-§3)

> **Reviewer:** sub-agent (A1)
> **Date:** 2026-05-12
> **Target:** `.gid/features/v04-unified-substrate/design.md` lines 1-369
> **Method:** full-depth review, focused on §0 TL;DR, §1 framing, §2 verified state, §3 terminal schema

## Summary

| Severity   | Count |
|------------|-------|
| 🔴 Critical   | 2     |
| 🟡 Important  | 4     |
| 🟢 Minor      | 3     |
| **Total**  | **9** |

**Recommendation**: Needs fixes before implementation — the FTS trigger bug (A1-3) and write journal contradiction (A1-6) are blocking. The naming inconsistencies (A1-1, A1-2) will cause real implementation confusion across sub-agents.

---

### FINDING-A1-1 🟡 Important — `node_type` / `edge_type` ghost columns used in §4 but absent from §3 schema

**Location**: §3.1 (schema DDL) vs §4.11 line 572, §4.14 line 686, §4.15 lines 718/736/740/818, §4.12 lines 615/576, §4.13 lines 656/657

**Issue**: The §3 terminal schema defines `node_kind TEXT NOT NULL` on `nodes` and `edge_kind TEXT NOT NULL` on `edges`. There is **no** `node_type` or `edge_type` column in either table. Yet §4 uses `node_type=` and `edge_type=` extensively:

- §4.11: `node_type='interoceptive', node_kind='domain'` — implies a two-level type discriminator (`node_type` + `node_kind`) that §3 doesn't have.
- §4.14: `node_type='metacog', node_kind='feedback'` — same two-level pattern.
- §4.15: `node_type='memory'`, `node_type='dimension'`, `node_type='topic'` — treated as a column.
- §4.12: `edge_type='evoked_by'`, `edge_type='aligns_with'` — but the column is `edge_kind`.
- §4.13: `edge_type='wm_contained'`, `edge_type='wm_snapshot_of'` — same.

This is a real ambiguity: either §3 is missing a `node_type` column (and the design intends a two-level node discriminator parallel to the two-level edge discriminator `edge_kind`+`predicate`), or §4 is using wrong column names. Two engineers would implement differently.

**Suggested fix**: Decide which is correct:
- **(A)** If `node_type` is intended: add `node_type TEXT NOT NULL` to §3.1 DDL and define the taxonomy (like `edge_kind` has a taxonomy table in §3.2). Update indexes.
- **(B)** If `node_type` is NOT intended: replace all `node_type=X, node_kind=Y` references in §4 with `node_kind=Y` (single discriminator), and replace all `edge_type=X` with `edge_kind=X` or `predicate=X` depending on intent.

Option (B) is simpler and consistent with the current schema. The two-level edge discriminator (`edge_kind` + `predicate`) already provides the expressivity; nodes may not need a second level.

---

### FINDING-A1-2 🟡 Important — `from_id`/`to_id` vs `source_id`/`target_id` naming inconsistency in edges

**Location**: §3.2 DDL (defines `source_id`, `target_id`) vs §4.15.2 line 744, §4.15.3 line 751, §6.1 line 1018, §6.8 line 1225

**Issue**: The `edges` table DDL in §3.2 defines columns `source_id` and `target_id`. However, multiple sections in §4 and §6 reference `from_id` and `to_id`:

- §4.15.2: `SELECT m.id FROM edges WHERE to_id=$loc`
- §4.15.3: `UNIQUE constraint on (from_id, to_id, edge_kind)`
- §6.1: `BumpAssociation { from_id: NodeId, to_id: NodeId, ... }`
- §6.8: `INSERT INTO edges (from_id, to_id, edge_kind)`

An implementer following §3.2 would write `source_id`/`target_id`; an implementer following §4.15 would write `from_id`/`to_id`. SQL would fail at runtime.

**Suggested fix**: Global replace `from_id` → `source_id` and `to_id` → `target_id` in §4 and §6 to match the §3.2 DDL. The Rust struct field names in §6.1's `WriteOp` can use whatever Rust convention makes sense, but the SQL and pseudo-SQL in the doc should be consistent with the DDL.

---

### FINDING-A1-3 🔴 Critical — FTS5 triggers use unsupported `WHERE id = ?` predicate

**Location**: §3.3, `nodes_fts_ad` and `nodes_fts_au` triggers

**Issue**: The FTS5 table `nodes_fts` uses triggers with `DELETE FROM nodes_fts WHERE id = old.id`. FTS5 virtual tables **do not support arbitrary WHERE clauses** — they only support `WHERE rowid = ?` or `WHERE nodes_fts MATCH '...'`. The `WHERE id = old.id` clause will fail at runtime with a SQLite error.

The design correctly identifies that `nodes.id` is TEXT (string UUID) and thus can't use FTS5's `content_rowid=` mode. But the proposed solution — treating `id` as a regular queryable column via `UNINDEXED` — doesn't work because FTS5's query interface is restricted regardless of column indexing.

**Suggested fix**: Use a **rowid mapping** approach:
1. Add an `INTEGER PRIMARY KEY` autoincrement column to `nodes` (or use SQLite's implicit `rowid`), and store that integer in a side-mapping or use it directly.
2. OR: Use a content-storing FTS5 table keyed by the implicit `rowid`, and maintain a `TEXT id → INTEGER rowid` lookup. The triggers would be:

```sql
CREATE TRIGGER nodes_fts_ad AFTER DELETE ON nodes BEGIN
    DELETE FROM nodes_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER nodes_fts_au AFTER UPDATE OF content, summary ON nodes BEGIN
    DELETE FROM nodes_fts WHERE rowid = old.rowid;
    INSERT INTO nodes_fts(rowid, content, summary)
    VALUES (old.rowid, new.content, new.summary);
END;

CREATE TRIGGER nodes_fts_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, content, summary)
    VALUES (new.rowid, new.content, new.summary);
END;
```

Then FTS queries join on `nodes.rowid = nodes_fts.rowid` (stable within a WAL epoch) or on a persisted mapping. The design's concern about VACUUM rowid instability (noted in the prose) applies only to `content=tablename` auto-sync mode — with manual triggers, you control the rowid mapping explicitly and VACUUM is not a threat because the triggers fire on every mutation.

Alternatively: use `content=''` (true contentless/external-content) with the special delete syntax: `INSERT INTO nodes_fts(nodes_fts, rowid, content, summary) VALUES('delete', old.rowid, old.content, old.summary)` — but this requires passing the *old* column values to the delete command.

---

### FINDING-A1-4 🟢 Minor — §4.15 references `nodes.body` but §3.1 defines `nodes.content`

**Location**: §4.15.1 line ~729

**Issue**: §4.15.1 says "`core_fact` is denormalized into `attributes` (in addition to being in `nodes.body`)" — but the §3.1 DDL names the column `content`, not `body`. The `WriteOp` enum in §6.1 also uses `body: String` as the field name for the Rust struct, which is fine for Rust naming, but the prose reference to the SQL column is wrong.

**Suggested fix**: Replace "in `nodes.body`" with "in `nodes.content`".

---

### FINDING-A1-5 🟡 Important — §7.2 misnumbered subsection "6.2.1" should be "7.2.1"

**Location**: §7.2, line ~1316

**Issue**: The subsection labeled `**6.2.1 Canonical vs clique**` appears inside §7.2 (Q2 — Entity surface forms are nodes). It should be `**7.2.1**`. The forward-reference at §4.2 line 414 correctly says "See §7.2.1" but the actual heading is `6.2.1`. An implementer looking for §7.2.1 won't find it.

**Suggested fix**: Rename `6.2.1` → `7.2.1`.

---

### FINDING-A1-6 🔴 Critical — §6.9 contradicts itself AND §8 T66 on write journal

**Location**: §6.9 line ~1250 vs §8.15 T66 line ~1574 vs §9 R9 line ~1636

**Issue**: Three places in the doc give contradictory statements about a write journal:

1. **§6.9** (body text): "**No write journal beyond SQLite's WAL.** A separate disk journal of pre-commit ops would be a 'WAL on top of WAL' — pointless duplication."
2. **§8.15 T66**: "Implement write journal (§6.9): append-only log of pending ops, fsync'd before queue ack — replays on crash recovery before accepting new writes."
3. **§9 R9**: "§6.9 write journal means in-flight ops survive restart"

§6.9 explicitly rejects a write journal. T66 tasks someone to implement one. R9 relies on it existing. An implementer would not know which is correct. This is a blocking ambiguity — the write journal decision affects crash-recovery semantics for every WriteOp.

**Suggested fix**: Decide one way:
- **(A) No journal (§6.9 wins)**: Delete T66. Update R9 to say "in-flight ops are lost on crash; callers retry via oneshot Err(QueueClosed)". This is the simpler, more honest design.
- **(B) Journal exists (T66 wins)**: Rewrite §6.9's "No write journal" paragraph to describe the journal design. Specify format, fsync strategy, replay semantics.

Recommendation: (A) — the §6.9 reasoning is sound. SQLite WAL already provides durability for committed batches. Uncommitted ops in the in-memory queue are inherently lost on crash regardless of a journal (the callers' oneshot channels are also lost). A pre-commit journal adds complexity for a recovery path that no caller can observe (since the caller's await handle is gone).

---

### FINDING-A1-7 🟢 Minor — §0 TL;DR says "10 tables" but §2 lists 10 data tables + omits FTS

**Location**: §0 line ~8, §2

**Issue**: §0 says the implementation grew into "**10 tables** (4 node-shaped, 5 edge-shaped, 1 FTS)". §2's table lists 10 rows categorized as 4 node + 5 edge + 1 "ext" (memory_embeddings). The FTS table (`memories_fts`) is not in §2's table. So either: (a) the count is 11 (10 data + 1 FTS), or (b) §0 is counting `memory_embeddings` as "FTS" which is incorrect — it's an embedding store, not full-text search.

Additionally, §2's summary says "4 active node-shaped tables, 5 active edge-shaped tables, 1 multi-model extension" which is internally consistent but doesn't mention FTS. The §0 count ("1 FTS") conflicts with §2's count ("1 multi-model extension").

**Suggested fix**: Align §0 with §2. Either "**11 tables** (4 node-shaped, 5 edge-shaped, 1 multi-model extension, 1 FTS)" or keep at 10 and clarify the accounting.

---

### FINDING-A1-8 🟡 Important — §2 specific numbers are unverified claims

**Location**: §2, lines ~88-103

**Issue**: §2 presents exact row counts as factual: `memories: 24624`, `memory_embeddings: 24467`, `entities: 2310`, `entity_relations: 6531`, `memory_entities: 9237`, `hebbian_links: 43710`, `synthesis_provenance: 72`. These are stamped "Verified current state (2026-05-12)" but there is no `[verify: ...]` annotation or reproducible query. The numbers are plausible but could be stale or fabricated.

Of particular note: `memory_embeddings: 24467` vs `memories: 24624` — a delta of 157 rows missing embeddings. This is plausible (failed embedding generation) but the gap is not explained.

The §4.16 evidence section DOES show reproducible commands (grep, find, sed) — §2 should follow the same standard.

**Suggested fix**: Add a reproducible verification block for §2 numbers:
```
$ sqlite3 rustclaw/engram-memory.db "SELECT COUNT(*) FROM memories"
24624
```
Or add `[verify: SELECT COUNT(*) FROM <table>]` annotations. Also explain the 157-row embedding gap.

---

### FINDING-A1-9 🟢 Minor — §3.1 `nodes` table has no index on `superseded_by`

**Location**: §3.1 DDL, indexes block

**Issue**: `superseded_by TEXT REFERENCES nodes(id)` is a column used in filtering (active nodes have `superseded_by IS NULL`). The existing code (storage.rs) filters with `(superseded_by IS NULL OR superseded_by = '')` on nearly every query. The §3.1 DDL has partial indexes for `deleted_at IS NULL` (`idx_nodes_deleted`) but no index covering `superseded_by`. Queries filtering active (non-superseded) nodes would need to scan.

**Suggested fix**: Add a partial index: `CREATE INDEX idx_nodes_active ON nodes(node_kind) WHERE deleted_at IS NULL AND superseded_by IS NULL;` — or fold `superseded_by IS NULL` into the existing `idx_nodes_deleted` filter.

---

<!-- FINDINGS -->

## What looks good

- **§3.1 `nodes` wide-table design is sound**: the NULL-heavy wide table with per-kind enforcement at application layer is the right call for SQLite (no JOIN tax on hot path). CHECK constraints on bounded fields are thorough.
- **§3.2 `edges` two-level discriminator (`edge_kind` + `predicate`)** is well-designed — stable outer taxonomy with open inner predicates avoids the rigidity trap. Partial UNIQUE indexes for associative/containment upsert are correct.
- **§3.4 `node_embeddings` multi-model extension** is clean — `(node_id, model)` PK, CASCADE delete, minimal surface area. Correctly keeps the hot-path single embedding inlined on `nodes.embedding`.
- **§5.1 Phase A is genuinely additive** — verified: all §3 DDL is `CREATE TABLE` (new tables only), no ALTER on existing tables. Phase A cannot break v0.2 data.
- **ISS-103 `occurred_at`/`created_at` semantics** are correctly carried from storage.rs into §3.1 — caller-supplied event time vs wall-clock ingest time, nullable, consistent with the existing migration.

## Applied

(None — awaiting human approval before apply phase.)
