# ISS-027: Ritual Workspace Guard — Prevent Rituals on Deprecated/Wrong Repos

**Status:** open (process bug / safety)
**Severity:** medium — caused 3 wasted ritual runs on 2026-04-22 and real risk of future corruption
**Related:** ISS-025 (cleanup), ISS-023 (consolidation that triggered the bug)
**Filed:** 2026-04-23 00:15 EDT

## Problem Statement

On 2026-04-22 at 23:26 EDT — 5 hours after ISS-023 consolidation made `engram/` the canonical repo — two (or three) rituals were launched against the deprecated `engram-ai-rust/` repo. Evidence:

```json
// engram-ai-rust/.gid/rituals/r-5fd66f.json
{
  "task": "Implement ISS-022: refactor ExtractedFact dimension fields...
           Project location: /Users/potato/clawd/projects/engram-ai-rust",
  "target_root": "/Users/potato/clawd/projects/engram-ai-rust"
}
```

The ritual system accepted this path without validation. Rituals ran for ~1m45s each, marked themselves "Done" on superficial verification, and produced essentially no effective work in the deprecated repo (just touched `.gid/graph.yml`). Meanwhile the ISS-022 task in the canonical monorepo was **not** addressed.

## Root Cause

**No validation layer between ritual launch and ritual execution.** Specifically:
1. `start_ritual` / ritual launcher trusts `target_root` input blindly
2. No canonical-repo marker exists (e.g., `.canonical` file)
3. No deprecated-repo marker mechanism exists
4. No cross-check against any registry of "active workspaces"

## Proposed Fix (multi-layered defense)

### Layer 1: Deprecated-repo marker file

Add a guard file to deprecated repo:
```
engram-ai-rust/.gid/DEPRECATED_DO_NOT_RITUAL
```

**Ritual launcher checks:** Before starting any ritual, read `{target_root}/.gid/DEPRECATED_DO_NOT_RITUAL`. If exists → abort with clear error message pointing to canonical path.

**Where to implement:**
- rustclaw `start_ritual` tool handler (primary entry)
- gid ritual phase runner (secondary, in case ritual bypasses tool)

### Layer 2: Canonical-repo registry

In `/Users/potato/rustclaw/rustclaw.yaml` or similar config, maintain an allowlist:
```yaml
ritual:
  allowed_workspaces:
    - /Users/potato/clawd/projects/engram
    - /Users/potato/clawd/projects/gid-rs
    # ...
```

Ritual launcher refuses any `target_root` not in this list. Requires explicit opt-in for new workspaces — annoying but correct.

### Layer 3: Dirty-tree refusal

A repo with uncommitted modifications at ritual start is a red flag. Ritual launcher should refuse unless `--force-dirty` explicitly provided.

```bash
# Pseudocode in ritual launcher
if !git_working_tree_clean(target_root) && !force_dirty:
    error("Refusing to run ritual on dirty working tree. Commit or stash first, or pass --force-dirty.")
```

(Both ritual runs on 2026-04-22 23:26 ran on a dirty tree — half-baked ISS-024 work — and still proceeded.)

### Layer 4: HEAD signature check (optional, advanced)

For each allowed workspace, record expected remote URL. Ritual launcher verifies:
```
git -C {target_root} remote get-url origin
```
matches the expected origin. Catches cases where a path accidentally resolves to an old clone.

## Implementation Steps

1. **Layer 1 (cheapest, highest ROI):** Add marker check in rustclaw `start_ritual` handler. ~20 lines of Rust. Add to ISS-025 Phase B.
2. **Layer 3:** Add dirty-tree check. ~30 lines. Also Phase B.
3. **Layer 2:** Add workspace allowlist to rustclaw.yaml. Medium effort (schema + loader + enforcement). Separate follow-up.
4. **Layer 4:** Origin signature check. Small effort, adds defense in depth. Follow-up.

## Acceptance Criteria

### Minimum viable fix (Phase B of ISS-025)
- [ ] Layer 1 implemented: `.gid/DEPRECATED_DO_NOT_RITUAL` check in `start_ritual`
- [ ] Layer 3 implemented: dirty-tree refusal (with override flag)
- [ ] Unit test: `start_ritual` fails when target has deprecated marker
- [ ] Unit test: `start_ritual` fails when target has dirty tree
- [ ] Manual smoke test: try to start ritual on `engram-ai-rust/` → rejected with clear message

### Full defense (follow-up)
- [ ] Layer 2: workspace allowlist in rustclaw.yaml
- [ ] Layer 4: remote origin signature check

## Why Not Rely on Humans Being Careful?

potato explicitly said: *"把这个错误给掰正"* — the goal is to prevent recurrence, not just clean up this instance. A process bug that fires once will fire again unless the process is fixed. Relying on "I'll remember to check the path" is not a system.

## Notes

- This bug was invisible until forensic analysis because rituals "succeeded" (marked Done). They just succeeded on the wrong repo. A silent wrong success is worse than a loud failure.
- Consider adding a ritual completion summary that includes `target_root` + HEAD commit SHA, making the path obvious in the success message.
