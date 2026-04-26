---
id: "ISS-027"
title: "Ritual Workspace Derivation — Ritual Must Know Where It Works"
status: closed
priority: P2
created: 2026-04-23
closed: 2026-04-23
severity: high
related: ["ISS-023", "ISS-025", "ISS-029"]
---
# ISS-027: Ritual Workspace Derivation — Ritual Must Know Where It Works

**Status:** closed (2026-04-23 — root-fix library work completed in gid-rs as ISS-029, commit `ffbedbc`)
**Severity:** high — caused 3 wasted ritual runs on 2026-04-22, and the root cause is architectural, not a missed check
**Related:** ISS-023 (consolidation that triggered the bug), ISS-025 (cleanup), **gid-rs ISS-029** (actual implementation)
**Filed:** 2026-04-23 00:15 EDT
**Rewritten:** 2026-04-23 01:02 EDT — v1 was patch-level (4 layers of guards); v2 reframes as root fix
**Closed:** 2026-04-23 — library fix landed in `gid-core` via gid-rs ISS-029

## Resolution

Root-fix implemented in `gid-rs` repo under its own issue number (ISS-029), commit `ffbedbc feat(iss-029): ritual launcher accepts WorkUnit, project_registry module` (2026-04-23 01:54 EDT). Implementation matches v2 design in this document:

- `gid-core/src/project_registry.rs` (699 lines, 22 tests) — YAML registry, XDG-compliant, resolve by name/alias with collision detection
- `gid-core/src/ritual/work_unit.rs` (269 lines, 8 tests) — `WorkUnit::{Issue, Feature, Task}` enum + `WorkUnitResolver` trait + `RegistryResolver` + `reject_target_root()` guard
- `gid-core/src/ritual/state_machine.rs` — `RitualState.work_unit: Option<WorkUnit>` (serde default for backwards compat)

**Library layer: done.** Issue closed.

### Remaining (follow-up, tracked separately)

rustclaw is a gid-core consumer and still calls the old `workspace`-argument path in its `start_ritual` tool handler. Migrating rustclaw to the `WorkUnit` API is an adopter-side task, not part of the root fix. Tracked in a separate rustclaw-side issue.

### Why this was confusing

- engram repo filed this as ISS-027; gid-rs repo implemented it as ISS-029. No cross-reference between the two numbers.
- The fix lives in the library (`gid-core` crate in gid-rs), not in engram (which is the repo where the bug manifested). Searching engram's tree for `WorkUnit` / `project_registry` / `resolver.rs` returns nothing because the code isn't there — it's in the dependency.

---

## Problem Statement

On 2026-04-22 at 23:26 EDT — 5 hours after ISS-023 consolidation made `engram/` the canonical repo — rituals ran against the deprecated `engram-ai-rust/` repo. Evidence:

```json
// engram-ai-rust/.gid/rituals/r-5fd66f.json
{
  "task": "Implement ISS-022: refactor ExtractedFact dimension fields...
           Project location: /Users/potato/clawd/projects/engram-ai-rust",
  "target_root": "/Users/potato/clawd/projects/engram-ai-rust"
}
```

The rituals ran ~1m45s each, marked themselves "Done" on superficial verification, produced no effective work. Meanwhile ISS-022 in the canonical monorepo was **not** addressed.

## Root Cause (revised)

v1 framed this as "ritual accepted a bad path." That's the **symptom**, not the cause. The real cause:

**Ritual's workspace is supplied as an argument, not derived from the work.**

The chain that failed:
1. User said: "work on ISS-022"
2. Agent translated to: `start_ritual(target_root="/path/agent/guessed")`
3. Agent's guess was stale (pre-consolidation context)
4. Ritual trusted the argument — it has no way to double-check because the argument is the only input

The `target_root` is **declared by the caller** rather than **derived from the unit of work** (issue, task). Any guard after step 4 (marker files, dirty-tree checks, allowlists) is a late warning, not a fix. As long as the ritual asks "where should I work?" instead of "what am I working on?", it will keep trusting whatever it's told.

## The Root Fix: Work-Unit-Driven Workspace

Invert the model:

**Before (current):**
```
start_ritual(target_root=PATH, task="Implement ISS-022")
→ ritual runs at PATH, task is context
```

**After (this fix):**
```
start_ritual(work_unit=ISS-022)
→ ritual reads the issue file
→ workspace = the repo where ISS-022 lives
→ no PATH argument needed; ambiguity is structurally impossible
```

**Key properties:**
1. **Every ritual is attached to a work unit** (issue, task, or explicitly-scoped scratch) — no free-floating `target_root`.
2. **Workspace is a function of the work unit**, not an input from the caller. `workspace(ISS-022) = path_of(ISS-022.file).repo_root`.
3. **Agent cannot supply a path that disagrees with the work unit.** If ISS-022 lives in `engram/`, ritual runs in `engram/`. Period. No override except explicit scratch mode.
4. **Rituals without a work unit are a different mode** — "scratch ritual": no project writes allowed (blocked at tool-scope level), useful for exploration/prototyping only.

## Why This Prevents 2026-04-22

The bug required:
- ISS-022 lives in `engram/` (canonical monorepo)
- Agent supplied `target_root=.../engram-ai-rust` (deprecated)
- System accepted the disagreement

Under work-unit-driven workspace:
- Agent calls `start_ritual(work_unit=ISS-022)`
- System reads `.gid/issues/ISS-022-*.md` — finds it in `engram/`
- Ritual runs in `engram/`, no override possible
- Bug cannot occur

The deprecated repo doesn't even need a marker file — it simply has no issues in its `.gid/issues/`, so no ritual can target it.

## Design

### Work Unit Resolution

A work unit is one of:
- **Issue:** `ISS-NNN` → resolve via `find <repos> -name "ISS-NNN*.md" -path "*/.gid/issues/*"`
- **Task:** `task-id` → resolve via `gid_tasks` in each known project
- **Feature:** `feature-name` → resolve via `.gid/features/{name}/` lookup
- **Scratch:** explicit `--scratch` — no workspace, no project writes

Resolution is **deterministic and unique**: exactly one repo must match. If zero → error ("work unit not found, create it first"). If >1 → error ("ambiguous; qualify with repo"). No silent pick.

### Known Repos Registry

Resolver needs a list of candidate repos to search. This lives in rustclaw config:
```yaml
ritual:
  known_projects:
    - /Users/potato/clawd/projects/engram
    - /Users/potato/clawd/projects/gid-rs
    - /Users/potato/rustclaw
    # ...
```

This is *not* an allowlist (v1's Layer 2) — it's a **discovery scope**. New repos must be registered before rituals can target their issues, which is correct (if the repo isn't in the registry, the agent shouldn't be working on it without explicit setup).

### Ritual Launcher API

**Old:**
```rust
start_ritual(task: String, target_root: Option<PathBuf>)
```

**New:**
```rust
enum WorkUnit {
    Issue(String),       // "ISS-022"
    Task(String),        // "impl-extractor"
    Feature(String),     // "ritual-workspace"
    Scratch,             // no project writes
}

start_ritual(work_unit: WorkUnit, task_description: String)
```

The `task_description` is still useful as human-readable context, but it is **not** used to derive workspace — that comes from `work_unit`.

### Migration of Existing Ritual State

Existing `.gid/rituals/*.json` files have `target_root` as source of truth. Add `work_unit` field going forward; for old rituals, leave `target_root` as-is (read-only legacy).

## Defense in Depth (retained from v1)

These are *not* the fix but are still worth having as safety nets for edge cases:

- **Dirty-tree refusal** — before starting, require clean git tree (unless `--force-dirty`). Catches the case where agent is mid-edit and confused about state. (v1 Layer 3)
- **Remote origin signature** — per `known_projects` entry, record expected `git remote get-url origin`. Verify match on ritual start. Catches accidentally-cloned duplicates. (v1 Layer 4)
- **Deprecated marker** — keep `.gid/DEPRECATED_DO_NOT_RITUAL` as a loud red flag for humans who navigate there manually, even though the resolver won't pick deprecated repos. Already placed in `engram-ai-rust/` as part of ISS-025. (v1 Layer 1, demoted)

v1's Layer 2 (allowlist) is subsumed by `known_projects` — same mechanism, different purpose (discovery, not veto).

## Implementation Steps

1. **Define `WorkUnit` enum and resolver** in `gid-core` (`crates/gid-core/src/workspace/resolver.rs`).
   - `resolve(work_unit, known_projects) -> Result<WorkspacePath>`
   - Unit tests covering: unique match, zero matches, multiple matches, scratch mode.
2. **Add `known_projects` to rustclaw config** (`rustclaw.yaml` + schema + loader).
3. **Rewrite `start_ritual` tool handler** in rustclaw to accept `WorkUnit` instead of `target_root`.
   - Call resolver → get workspace → pass to ritual phase runner.
   - Ritual state JSON gains `work_unit` field.
4. **Add dirty-tree check** in resolver (Layer 3 from v1).
5. **Add origin signature check** in resolver (Layer 4 from v1).
6. **Smoke tests:**
   - `start_ritual(ISS-022)` → resolves to `engram/`, runs there
   - `start_ritual(ISS-XXX-doesnt-exist)` → rejects with "not found"
   - Issue file present in two repos → rejects as ambiguous
   - Dirty tree → rejects unless forced

## Acceptance Criteria

### Core (root fix)
- [ ] `WorkUnit` enum + resolver in `gid-core`
- [ ] `known_projects` config + schema
- [ ] `start_ritual` takes `WorkUnit`, not `target_root`
- [ ] Ritual state JSON records `work_unit`
- [ ] Unit test: resolver returns correct path for unique issue
- [ ] Unit test: resolver errors on zero/multi matches
- [ ] Smoke test: launching ritual for ISS-022 runs in `engram/`, never `engram-ai-rust/`

### Defense in depth
- [ ] Dirty-tree refusal (with `--force-dirty` override)
- [ ] Remote origin signature verification
- [ ] `.gid/DEPRECATED_DO_NOT_RITUAL` as human-facing marker (already in place from ISS-025)

### Scratch mode
- [ ] `WorkUnit::Scratch` supported; writes to project paths blocked at tool-scope level

## Why Not Just Patch v1?

v1's 4-layer defense works — it would have caught the 2026-04-22 bug. But:

- It leaves the **asking-the-wrong-question** architecture in place. Next year, a different edge case will slip past all 4 layers.
- It treats the agent's `target_root` declaration as authoritative input that needs checking, when it should never have been input at all.
- Every new project requires remembering to update the allowlist, the marker files, etc. Easy to miss. Under work-unit-driven, registering a project in `known_projects` is the *only* step — everything else is automatic.
- Engineering philosophy (SOUL.md): "Root fix, not patch." A bug caught by 4 defensive layers is still a bug that almost happened.

## Notes

- The v1 document remains at `ISS-027-ritual-workspace-guard.md` for history. This file (`ISS-027-ritual-workspace-derivation.md`) is the active design.
- After this ships, the 2026-04-22 class of bug becomes structurally impossible, not just unlikely.
- Open question: how does this interact with cross-project work (e.g., a feature spanning `engram/` and `gid-rs/`)? Proposal: a cross-project ritual is a sequence of single-project sub-rituals, each with its own work unit. Don't support "two workspaces at once" — it defeats the whole model.
