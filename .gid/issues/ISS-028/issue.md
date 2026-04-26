---
id: "ISS-028"
title: "Issue status drift between `.gid/graph.db` and `.gid/issues/*.md`"
status: open
priority: P2
created: 2026-04-23
severity: low
related: ["ISS-023", "ISS-027"]
---
# ISS-028: Issue status drift between `.gid/graph.db` and `.gid/issues/*.md`

**Status:** open
**Severity:** low — not blocking work, but erodes graph usefulness over time
**Related:** ISS-023 (consolidation), ISS-027 (ritual workspace)
**Filed:** 2026-04-23

## Summary

Issue status is currently tracked in two places, and they are out of sync:

1. **Canonical source (in practice):** `**Status:**` field at the top of each `.gid/issues/ISS-NNN*.md` file.
2. **Graph database:** `.gid/graph.db` has a `nodes` table with a `status` column, but only ISS-001 exists there. ISS-015 through ISS-028 are **not represented in the graph at all**.

Last `graph.db` mtime: 2026-04-19. Issue files have evolved daily since then. The graph has been effectively frozen for 4+ days with respect to issue tracking.

## Evidence

```
$ sqlite3 .gid/graph.db "SELECT id, status FROM nodes WHERE id LIKE 'iss-%' OR id LIKE 'ISS-%' ORDER BY id;"
iss-001|done
```

Only one issue in the graph. All other ISS-xxx entries live purely as markdown files.

## Impact

- `gid tasks` / `gid query` / any graph-based issue lookup returns stale or empty results for current work.
- New agents/sessions that trust the graph as the source of truth will miss ~12 open issues.
- There is no single-query answer to "what issues are open?" — one has to `grep` markdown files.
- Cross-issue dependency tracking via graph edges is impossible when the nodes don't exist.

## Root Cause (first-principles)

No automated sync mechanism between issue markdown files and the graph. Issues are created by:
- Agents writing markdown files directly
- Manual `gid_add_task` calls (rare, usually forgotten)

There is no hook/watcher/importer that keeps them aligned. The divergence is monotonic — it only gets worse.

## Options (not decided)

**Option A — Markdown as single source of truth, drop issue nodes from graph.**
- Pros: simplest; eliminates the drift problem by eliminating the duplicate.
- Cons: loses graph querying (`gid tasks`, dependency edges between issues).
- Cost: near-zero; just stop pretending the graph tracks issues.

**Option B — Issue importer / bidirectional sync.**
- Scan `.gid/issues/*.md` → parse `**Status:**` + `**Related:**` → upsert into graph nodes/edges.
- Run on commit hook or as part of ritual/heartbeat.
- Pros: graph stays useful; single query answers "what's open".
- Cons: parser to build+maintain; risk of conflicting edits (file vs graph write).

**Option C — Graph as single source of truth, generate markdown from graph.**
- Pros: one write path (graph), markdown is derived/rendered.
- Cons: large refactor; breaks current workflow where agents edit markdown directly; issue narratives are rich text that doesn't round-trip well through structured fields.
- Cost: high.

## Recommendation

Lean Option A or Option B. Option C is overkill and fights current agent behavior.

- **Option A** if we decide issues don't need graph querying (they rarely have structured cross-references today; `Related:` lines in markdown already work).
- **Option B** if we want `gid tasks` / `gid_query_impact` to work for issues, worth building a ~200-line importer.

Decision deferred until someone actually needs the graph view of issues. Until then: **markdown is the source of truth**, graph is stale.

## Next Steps

- [ ] Decide Option A vs B (not urgent)
- [ ] If B: write `gid issue-import` subcommand that scans `.gid/issues/*.md` and upserts nodes
- [ ] Either way: document the chosen source of truth in `.gid/docs/` so future agents don't waste time checking both places

## Notes

- Discovered 2026-04-23 while verifying ISS-025 closure — noticed `gid_tasks` returned nothing for ISS-025/ISS-027, then checked graph.db directly and found only `iss-001`.
- Not urgent because markdown-only works fine for current workflow. Filed so we don't rediscover the drift six months from now and wonder how bad it got.
