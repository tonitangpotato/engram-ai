---
id: "ISS-028"
title: "Graph node status drift — issues, tasks, and code reality out of sync"
status: open
priority: P1
created: 2026-04-23
updated: 2026-04-29
severity: medium
related: ["ISS-023", "ISS-027", "ISS-044"]
---
# ISS-028: Graph node status drift — issues, tasks, and code reality out of sync

**Status:** open
**Severity:** medium — actively misleads planning tools (`gid_plan`, `gid_advise`) and review docs
**Priority:** P1 (was P2, raised 2026-04-29 — now actively causing wrong reads)
**Related:** ISS-023 (consolidation), ISS-027 (ritual workspace), ISS-044 (mig backfill orchestrator — example of task-side drift)
**Filed:** 2026-04-23
**Updated:** 2026-04-29 — scope expanded to cover task-node drift after concrete misread

## Summary

Graph node status drifts away from reality across **three node kinds**, all caused by the same root issue (no automated sync). Originally filed for issue-node drift; expanded 2026-04-29 after task-node drift produced a concrete wrong read in a status review.

The three drift surfaces:

1. **Issue-node drift** (original scope, 2026-04-23): markdown files at `.gid/issues/ISS-NNN/issue.md` evolve, graph nodes don't. Only ISS-001 exists in graph; ISS-015..ISS-068 live purely as markdown.
2. **Task-node drift** (added 2026-04-29): `.gid-v03-context/graph.db` task nodes report `status=todo` while the underlying code is fully implemented. Concrete example: `task:mig-wire-backfill-orchestrator` is `todo` but `crates/engramai-migrate/src/cli.rs:663 run_backfill` is wired and complete (corresponding ISS-044 is `in_review`).
3. **Code-vs-graph drift** (added 2026-04-29): code-layer nodes from `gid_extract` can lag behind file moves/renames. Lower priority but same family.

## Evidence

### Issue-node drift (original)

```
$ sqlite3 .gid/graph.db "SELECT id, status FROM nodes WHERE id LIKE 'iss-%' OR id LIKE 'ISS-%' ORDER BY id;"
iss-001|done
```

Only one issue in the graph. ISS-015..ISS-068 live purely as markdown.

### Task-node drift (new, 2026-04-29)

```
$ sqlite3 .gid-v03-context/graph.db \
    "SELECT id, status FROM nodes WHERE id='task:mig-wire-backfill-orchestrator';"
task:mig-wire-backfill-orchestrator|todo
```

But `crates/engramai-migrate/src/cli.rs:663` (`run_backfill`) is a complete, wired implementation that iterates `memories` from v0.2 and runs `PipelineRecordProcessor` per record. ISS-044 (the umbrella issue) is already `in_review`. The graph node never got the memo.

**Concrete consequence:**
- `tasks/2026-04-29-engram-v03-status.md` v1 read the graph and concluded "run_backfill is a stub" → wrong.
- `gid_plan` reports `task:mig-followup-graph-delta-borrowed-tx` as critical-path ETA = 60 turns, weighted by `task:mig-wire-backfill-orchestrator` still being open. The estimate is built on stale state.
- Required v2 rewrite of the status doc. This is the second time graph drift has produced a confidently-wrong document this month.

## Impact (updated)

- `gid_tasks` / `gid_plan` / `gid_advise` produce misleading critical paths.
- Status review documents trust the graph and have to be retracted (concrete: 2026-04-29 v1 retracted).
- "What's actually shippable?" cannot be answered from the graph alone — must cross-check code.
- Cross-issue/task dependency tracking via graph edges is unreliable when status is wrong.
- New agents starting fresh trust the graph and act on bad info; this is the #1 onboarding hazard for autopilot/sub-agent work.

## Root Cause (first-principles)

There is no automated sync mechanism between **any** of:
- Markdown issue files ↔ graph issue nodes
- Code reality (commits, file content) ↔ graph task nodes
- Filesystem layout (file moves, deletions) ↔ graph code-layer nodes

Status changes happen in:
- Markdown files (agents edit `**Status:**` field directly)
- Code (agents/PRs land implementations without touching the graph)
- `gid_complete` / `gid_update_task` calls (rare; relies on agent remembering)

The drift is **monotonic** — it only widens until someone manually reconciles. There is no canary, no PR-time check, no nightly job.

## Options

### A. Markdown + code as source of truth, stop pretending graph tracks issue/task status
- **Issues:** drop issue nodes from graph entirely; `gid_tasks` doesn't try to list issues.
- **Tasks:** demote graph task `status` to "advisory only" — print a warning that it may be stale; tools should not trust it.
- **Pros:** zero implementation cost; matches actual current behavior.
- **Cons:** loses `gid_plan` / `gid_advise` usefulness for issue/task work; planning tools become decorative.

### B. One-way importers (markdown/code → graph)
- **Issue importer:** scan `.gid/issues/*.md`, parse frontmatter (`status:`, `priority:`, `related:`, `depends_on:`), upsert graph nodes + edges.
- **Task importer:** when a task's referenced source file or function is materially complete (heuristic: file exists + non-stub + tests pass), suggest `status=done`.
- **Run:** on `gid_validate`, on PR commit, or as part of ritual/heartbeat.
- **Pros:** graph stays useful; planning tools work.
- **Cons:** parsers + heuristics to build and maintain. Task-side heuristic is fuzzy ("is this code done?" is not a clean signal).

### C. Graph as single source of truth, render markdown
- Massive refactor, fights current agent workflow, doesn't compose with issue narratives that are rich text. **Rejected.**

### D. Hybrid (recommended)
- **Issues — Option B importer.** Cheap (~200 lines), markdown frontmatter has structured fields already, parse is straightforward. Run on `gid_validate` and as a ritual phase.
- **Tasks — manual `gid_complete` + a drift detector** (rather than auto-importer). Detector runs in `gid_validate`: for each task with `file_path` metadata, check the file exists and has > N lines / contains the named function; if so but task is `todo`, warn. Don't auto-update — false positives (e.g., scaffolding without real impl) would corrupt state.
- **Code-layer — re-extract on schedule.** `gid_extract` is idempotent; run weekly or after large refactors.

## Recommendation

**Hybrid (Option D).** Cheap importer for issues + drift detector + manual completion for tasks + scheduled re-extract for code.

This isolates the only fuzzy bit (task completion heuristic) and keeps it as a *warning*, not an automated mutator. Auto-mutating task status from "this file looks done" is too risky.

## Next Steps

- [ ] **Phase 1 — Drift detector (priority).** Add `gid_validate` warnings for:
  - Issue markdown files whose `status:` frontmatter doesn't match graph node status (or graph node missing entirely)
  - Task nodes with `file_path` where the file is non-trivial but task is still `todo`
  - Code nodes whose `file_path` no longer exists
- [ ] **Phase 2 — Issue importer.** `gid issue-import` subcommand that scans `.gid/issues/*/issue.md`, upserts nodes + `related`/`depends_on` edges from frontmatter.
- [ ] **Phase 3 — Doc the policy.** Write `.gid/docs/source-of-truth.md` declaring: markdown is canonical for issues, code is canonical for task completion, graph is a lossy projection.
- [ ] **Phase 4 — Ritual integration.** Run drift detector at start of every ritual phase; refuse to plan if drift count > threshold.
- [ ] **Immediate (today, separate from phases):** manually run `gid_complete` on `task:mig-wire-backfill-orchestrator` and any other task in the v0.3 graph whose code is verified complete. This is graph housekeeping, not a fix — but it un-blocks the v0.3 LoCoMo verification path.

## Why Priority Was Raised (2026-04-29)

When this was filed (2026-04-23) it was P2/low because issue-node drift didn't actively cause wrong decisions — markdown was canonical and people grep'd it.

Today the drift produced a concretely wrong document (`tasks/2026-04-29-engram-v03-status.md` v1 said "run_backfill is a stub", retracted 2 hours later when source was checked). With autopilot/sub-agent workflows trusting `gid_plan` to choose what to work on, drift now actively misroutes work. P1.

## Notes

- 2026-04-23: Discovered while verifying ISS-025 closure — `gid_tasks` returned nothing for ISS-025/ISS-027.
- 2026-04-29: Discovered task-side drift while writing v0.3 status review. v1 review retracted because graph said `task:mig-wire-backfill-orchestrator` was todo but code was wired.
- This is also the same family as `cite-before-claim` skill (skill-level patch for unverified factual claims) — the pattern of "trust a structured datastore that's actually stale" recurs across systems.
