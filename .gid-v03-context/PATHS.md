# V0.3 Manual Graph Build — Path Lock-in

> **Created:** 2026-04-24 22:25
> **Last updated:** 2026-04-25 11:55 (Rule 2 superseded — see §"Superseded guidance" at bottom)

## Canonical Paths

| Concept | Path |
|---|---|
| **engram repo root (real)** | `/Users/potato/clawd/projects/engram/` (inode 25877947) |
| **engram repo root (symlinked alias)** | `/Users/potato/rustclaw/projects/engram/` → same inode |
| `rustclaw/projects` itself | symlink → `/Users/potato/clawd/projects` (so the whole subtree is shared) |
| **gid registry entry** | `engram` → `/Users/potato/clawd/projects/engram` (use this in tools) |
| **v0.3 working graph DB** | `/Users/potato/clawd/projects/engram/.gid-v03-context/graph.db` (8.6MB, in git as of 662fb8f) |
| **v0.3 working graph extract-meta** | `/Users/potato/clawd/projects/engram/.gid-v03-context/extract-meta.json` |
| **MAIN graph DB (do NOT touch for v0.3 work)** | `/Users/potato/clawd/projects/engram/.gid/graph.db` |
| **5 v0.3 design docs** | `/Users/potato/clawd/projects/engram/.gid/features/v03-{retrieval,graph-layer,resolution,benchmarks,migration}/` |
| **master requirements** | `/Users/potato/clawd/projects/engram/.gid/docs/requirements-v03.md` |

## Rules for This Build

1. **All graph writes MUST go to `.gid-v03-context/graph.db`**, NOT `.gid/graph.db`.

2. **Use RustClaw's native `gid_*` tools with the `graph_path` parameter.** The `graph_path` arg overrides project default and writes directly to the specified DB file.
   - Pass `graph_path: "/Users/potato/clawd/projects/engram/.gid-v03-context/graph.db"` (absolute path) on every call.
   - Tools to use: `gid_add_task` (for any node — task/feature/component), `gid_add_edge`, `gid_update_task`, `gid_complete`, `gid_read`, `gid_tasks`, `gid_validate`, `gid_query_impact`, `gid_query_deps`.
   - `gid_add_task` writes `node_type` correctly when you pass it explicitly (e.g. `node_type: "feature"`).
   - **For atomic multi-op batches**, native tools don't have a batch endpoint yet — fall back to the CLI:
     ```bash
     cd /Users/potato/clawd/projects/engram && \
     ~/.cargo/bin/gid --graph .gid-v03-context/graph.db --backend sqlite edit-graph '[ ... ops ... ]'
     ```
     **⚠️ CLI caveat (verified 2026-04-25):** the CLI's `edit-graph add_node` op silently writes `node_type='unknown'` regardless of the input field. Symptom: nodes are invisible to `gid_read --layer project`. **Workaround:** prefer native tools; if you must use the CLI, verify with `sqlite3 ... "SELECT node_type FROM nodes WHERE id=..."` after every write and fix via `gid_update_task` (which sets node_type correctly).

3. **NEVER run `gid_extract` again** — that's what created the 107MB `.wrong-v03-write` mess. We already have code nodes.

4. **NEVER call `gid_design --parse` with unreviewed YAML.**

5. **Backup before each feature build:**
   ```bash
   cp .gid-v03-context/graph.db .gid-v03-context/graph.db.before-{feature}-$(date +%Y%m%d-%H%M%S)
   ```

6. **Verify after each batch:**
   ```bash
   sqlite3 .gid-v03-context/graph.db "SELECT node_type, COUNT(*) FROM nodes GROUP BY node_type;"
   ```
   Expect counts to GROW by exactly the number planned, never explode.

## Past Mess-Up Evidence

- `.gid/graph.db.wrong-v03-write-20260424-181500` is **107MB** (vs current 8.6MB working db) → previous attempt likely ran `gid extract` on the wrong directory, or auto-merged extract into main DB. Avoid both.
- 04-24 17:50 backup: `.gid/graph.db.before-v03-build-20260424-175003` (1.5MB) — pre-attempt state.

## Current `.gid-v03-context/graph.db` State (as of 2026-04-25 11:54, post-T1)

```
node_type | count
----------|------
code      | 3039
component | 23
feature   | 13  (10 auto-discovered by gid infer + 3 v0.3 manual: v03-retrieval, retrieval-classification, retrieval-execution)
TOTAL     | 3075
edges     | 7887  (7885 original + 2 new subtask_of)
```

**Still missing (to be built per `v03-retrieval-build-plan.md`):**
- T2: 17 `code:planned:*` modules + 17 `defined_in` edges
- T3: 16 task nodes + 16 requirement nodes (14 GOAL + 2 GUARD) + ~45 implements/satisfies edges
- T4: cross-feature `depends_on` edges (waiting for `feature:v03-graph-layer`, `feature:v03-resolution` to exist)
- 4 other v0.3 features: graph-layer, resolution, benchmarks, migration

---

## Superseded guidance

The following rule was in effect from 2026-04-24 22:25 to 2026-04-25 11:55. **It was wrong** — kept here for historical traceability.

### ❌ SUPERSEDED Rule 2 (original):

> **DO NOT use MCP `gid_*` tools** — they default to `.gid/graph.db` with no override for DB path. Even with `project: ...`, they write to `<project>/.gid/graph.db`.
>
> **USE CLI directly** with explicit `--graph` flag:
> ```bash
> cd /Users/potato/clawd/projects/engram && \
> ~/.cargo/bin/gid --graph .gid-v03-context/graph.db --backend sqlite <command>
> ```

### Why it was wrong:

- RustClaw's built-in `gid_*` tools accept a `graph_path` parameter that **does** override the workspace default — verified 2026-04-25 by writing 3 feature nodes via `gid_add_task` to `.gid-v03-context/graph.db` directly.
- The CLI is **less reliable** than the native tools because `edit-graph add_node` silently fails to write `node_type` correctly (writes `'unknown'`). The native `gid_add_task` writes node_type correctly.
- The original guidance may have been correct for an older RustClaw build where `graph_path` didn't exist or wasn't honored. Current build (2026-04-25, RustClaw v0.1.0) has it working.

### Migration

- **Replaced by:** new Rule 2 (above) — "Use native `gid_*` tools with `graph_path` parameter".
- **CLI is now a fallback** only when batch atomicity is required (and even then, with the `unknown`-node_type caveat).
