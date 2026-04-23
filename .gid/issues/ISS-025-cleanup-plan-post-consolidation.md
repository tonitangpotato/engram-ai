# ISS-025: Port Post-Consolidation Changes from `engram-ai-rust/` to `engram/`

**Status:** open
**Severity:** medium — straightforward porting work, but must not drop changes
**Related:** ISS-023 (consolidation), ISS-024 (WIP), ISS-026 (lint), ISS-027 (ritual guard)
**Filed:** 2026-04-23 00:29 EDT

## Summary

After ISS-023 consolidated `engram-ai-rust/` → `engram/` monorepo at 18:30 EDT on 2026-04-22, work continued on the deprecated repo for ~5 hours. Port **all** of those changes into the monorepo, then deprecate the old repo.

## What Needs Porting (complete inventory)

### A. Committed after consolidation
**1 commit:** `71f3654 chore(lint): ISS-019 Step 8.5 — clippy cleanup` (23:11 EDT)
- 31 files, 1529-line diff (~424 lines outside `.gid/graph.yml`)
- Real code fixes in `src/memory.rs` (redundant if-branch, struct update syntax), `src/metacognition.rs` (Iterator::flatten), `src/enriched.rs` (rustdoc)
- `#[allow(clippy::…)]` annotations on test modules and examples
- `.gid/graph.yml` key reordering (non-semantic, skip)

### B. Uncommitted modified files (in `git status`)
- `src/dimensions.rs` — ISS-024 design changes
- `src/lib.rs` — ISS-024 new module wiring
- `src/memory.rs` — ISS-024 read-path changes
- `Cargo.toml` — ISS-024 adds `two_timer` + `lru` deps
- `.gid/graph.yml` — ritual key reordering (skip, same as above)

### C. Uncommitted new files (untracked)
- `src/dimension_access.rs` — ISS-024 new module
- `src/temporal_dim.rs` — ISS-024 new module
- `.gid/graph.yml.post-iss024-design` — ISS-024 design snapshot
- `.gid/issues/ISS-019-dimensional-metadata-write-gap/VERIFY_REPORT.md`
- `.gid/issues/ISS-023-repo-consolidation-monorepo.md` (already exists in monorepo, skip)
- `.gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/` (full dir)
- `.gid/rituals/r-5fd66f.json`, `r-5ff35a.json`, `r-e9410e.json` (wasted ritual runs, archive only)

### D. Files on disk but not in git (examples not declared in Cargo.toml)
- `examples/iss019_smoke_pilot.rs`
- `examples/synthesis_bench.rs`
- (`examples/kc_e2e_real.rs` already exists in monorepo as `examples/legacy/kc_e2e_real.rs.disabled`)

## Path Mapping

Deprecated layout → monorepo layout:

| Deprecated | Monorepo |
|---|---|
| `src/…` | `crates/engramai/src/…` |
| `tests/…` | `crates/engramai/tests/…` |
| `examples/…` | `crates/engramai/examples/…` |
| `Cargo.toml` | `crates/engramai/Cargo.toml` |
| `.gid/…` | `.gid/…` (same, at monorepo root) |

## Strategy

**Committed changes (A):** use `git format-patch` + `git am` with path filter — not cherry-pick, because paths differ.

**Uncommitted changes (B, C, D):** plain `cp` for new files, manual re-application for modified files (they're ISS-024 WIP, ~150 lines total, easier to read+apply than patch).

## Execution Plan

### Step 1: Prepare workspace
```bash
cd /Users/potato/clawd/projects/engram-ai-rust
git status  # confirm dirty state matches expected inventory above
```

### Step 2: Port committed lint changes (71f3654) via patch+rewrite paths

```bash
cd /Users/potato/clawd/projects/engram-ai-rust

# Export as patch, excluding .gid/graph.yml (non-semantic noise)
git format-patch -1 71f3654 --stdout -- \
    ':(exclude).gid/graph.yml' \
    > /tmp/71f3654.patch

# Rewrite paths in patch: src/ → crates/engramai/src/, tests/ → crates/engramai/tests/, examples/ → crates/engramai/examples/
sed -E \
    -e 's| a/src/| a/crates/engramai/src/|g' \
    -e 's| b/src/| b/crates/engramai/src/|g' \
    -e 's| a/tests/| a/crates/engramai/tests/|g' \
    -e 's| b/tests/| b/crates/engramai/tests/|g' \
    -e 's| a/examples/| a/crates/engramai/examples/|g' \
    -e 's| b/examples/| b/crates/engramai/examples/|g' \
    -e 's|^--- a/src/|--- a/crates/engramai/src/|g' \
    -e 's|^\+\+\+ b/src/|+++ b/crates/engramai/src/|g' \
    -e 's|^--- a/tests/|--- a/crates/engramai/tests/|g' \
    -e 's|^\+\+\+ b/tests/|+++ b/crates/engramai/tests/|g' \
    -e 's|^--- a/examples/|--- a/crates/engramai/examples/|g' \
    -e 's|^\+\+\+ b/examples/|+++ b/crates/engramai/examples/|g' \
    /tmp/71f3654.patch > /tmp/71f3654-monorepo.patch

# Apply to monorepo
cd /Users/potato/clawd/projects/engram
git am --3way /tmp/71f3654-monorepo.patch
# If conflicts → git am --abort, investigate, fix manually
```

**Rollback:** `git am --abort` (if mid-apply), or `git reset --hard HEAD~1` (if applied but wrong).

### Step 3: Port uncommitted ISS-024 WIP code

```bash
# 3a. Copy new files
cp /Users/potato/clawd/projects/engram-ai-rust/src/dimension_access.rs \
   /Users/potato/clawd/projects/engram/crates/engramai/src/
cp /Users/potato/clawd/projects/engram-ai-rust/src/temporal_dim.rs \
   /Users/potato/clawd/projects/engram/crates/engramai/src/

# 3b. Generate diff of modified files, rewrite paths, save as ISS-024 patch (do NOT apply yet — compile status unknown)
cd /Users/potato/clawd/projects/engram-ai-rust
git diff -- src/dimensions.rs src/lib.rs src/memory.rs Cargo.toml > /tmp/iss-024-modified.diff

sed -E \
    -e 's| a/src/| a/crates/engramai/src/|g' \
    -e 's| b/src/| b/crates/engramai/src/|g' \
    -e 's|^--- a/src/|--- a/crates/engramai/src/|g' \
    -e 's|^\+\+\+ b/src/|+++ b/crates/engramai/src/|g' \
    -e 's| a/Cargo.toml| a/crates/engramai/Cargo.toml|g' \
    -e 's| b/Cargo.toml| b/crates/engramai/Cargo.toml|g' \
    -e 's|^--- a/Cargo.toml|--- a/crates/engramai/Cargo.toml|g' \
    -e 's|^\+\+\+ b/Cargo.toml|+++ b/crates/engramai/Cargo.toml|g' \
    /tmp/iss-024-modified.diff > /tmp/iss-024-monorepo.patch

# 3c. Stage the patch into monorepo for later application (don't apply yet)
mkdir -p /Users/potato/clawd/projects/engram/.gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/wip/
cp /tmp/iss-024-monorepo.patch \
   /Users/potato/clawd/projects/engram/.gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/wip/

# 3d. Write a README explaining state
cat > /Users/potato/clawd/projects/engram/.gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/wip/README.md <<'EOF'
# ISS-024 WIP snapshot from deprecated repo

Ported 2026-04-23 from engram-ai-rust/ after ISS-023 consolidation.

## Contents
- `iss-024-monorepo.patch` — diff of src/dimensions.rs, src/lib.rs, src/memory.rs, Cargo.toml (paths rewritten for monorepo)
- `src/dimension_access.rs` and `src/temporal_dim.rs` were COPIED directly (new files)

## Status
Uncommitted WIP. Compile status in deprecated repo was unknown at port time.

## How to resume
1. `git apply .gid/issues/ISS-024-.../wip/iss-024-monorepo.patch`
2. `cargo check -p engramai`
3. If green → continue ISS-024 implementation. If red → read compiler errors, likely missing imports for dimension_access/temporal_dim.
EOF
```

### Step 4: Port issue docs + design snapshot

```bash
# ISS-024 full directory
cp -r /Users/potato/clawd/projects/engram-ai-rust/.gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/* \
      /Users/potato/clawd/projects/engram/.gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/

# ISS-019 verify report
cp /Users/potato/clawd/projects/engram-ai-rust/.gid/issues/ISS-019-dimensional-metadata-write-gap/VERIFY_REPORT.md \
   /Users/potato/clawd/projects/engram/.gid/issues/ISS-019-dimensional-metadata-write-gap/

# Design snapshot (belongs with ISS-024)
cp /Users/potato/clawd/projects/engram-ai-rust/.gid/graph.yml.post-iss024-design \
   /Users/potato/clawd/projects/engram/.gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/wip/graph.yml.post-iss024-design
```

### Step 5: Port sample examples (decide keep-or-skip)

```bash
# These are NOT declared in Cargo.toml (deprecated or monorepo) — they're reference code
cp /Users/potato/clawd/projects/engram-ai-rust/examples/iss019_smoke_pilot.rs \
   /Users/potato/clawd/projects/engram/crates/engramai/examples/
cp /Users/potato/clawd/projects/engram-ai-rust/examples/synthesis_bench.rs \
   /Users/potato/clawd/projects/engram/crates/engramai/examples/

# Note: examples/kc_e2e_real.rs already exists in monorepo as examples/legacy/kc_e2e_real.rs.disabled. Skip.
```

### Step 6: Archive wasted ritual state files (evidence for ISS-027)

```bash
mkdir -p /Users/potato/clawd/projects/engram/.gid/archive/iss-027-ritual-bug-evidence/
cp /Users/potato/clawd/projects/engram-ai-rust/.gid/rituals/r-5fd66f.json \
   /Users/potato/clawd/projects/engram-ai-rust/.gid/rituals/r-5ff35a.json \
   /Users/potato/clawd/projects/engram-ai-rust/.gid/rituals/r-e9410e.json \
   /Users/potato/clawd/projects/engram/.gid/archive/iss-027-ritual-bug-evidence/

cat > /Users/potato/clawd/projects/engram/.gid/archive/iss-027-ritual-bug-evidence/README.md <<'EOF'
# Evidence for ISS-027 (ritual workspace guard bug)

These ritual state files were created in engram-ai-rust/.gid/rituals/ on
2026-04-22 at 23:26 EDT — ~5 hours after ISS-023 consolidation made
engram/ the canonical repo. They demonstrate the ritual launcher
accepting a deprecated target_root without validation.

See ISS-027 for the fix design.
EOF
```

### Step 7: Verify monorepo still builds

```bash
cd /Users/potato/clawd/projects/engram

# Record pre-port baseline (should match 71f3654 verify comment: 1176 passed)
# cargo test -p engramai 2>&1 | tail -5

# After steps 2+5 (committed lint + new examples), run:
cargo build --workspace
cargo test -p engramai 2>&1 | tail -5

# Expected: tests still pass. The lint commit should not change test results.
```

**Rollback:** If Step 2 landed broken, `git reset --hard HEAD~1`. If Step 5 examples don't compile, `rm crates/engramai/examples/iss019_smoke_pilot.rs crates/engramai/examples/synthesis_bench.rs`.

### Step 8: Commit the ports

```bash
cd /Users/potato/clawd/projects/engram

# Step 2 already made a commit via git am. Check it:
git log -1

# Additional commit for steps 3-6 (docs, WIP snapshot, archived evidence, untracked examples)
git add .gid/issues/ISS-024-* .gid/issues/ISS-019-*/VERIFY_REPORT.md \
        .gid/archive/iss-027-ritual-bug-evidence/ \
        crates/engramai/src/dimension_access.rs \
        crates/engramai/src/temporal_dim.rs \
        crates/engramai/examples/iss019_smoke_pilot.rs \
        crates/engramai/examples/synthesis_bench.rs
git commit -m "chore: port uncommitted work from deprecated engram-ai-rust (ISS-025)

- ISS-024 WIP: new modules (dimension_access.rs, temporal_dim.rs) and
  modification patch staged in .gid/issues/ISS-024-.../wip/
- ISS-024 issue docs ported
- ISS-019 VERIFY_REPORT.md ported
- ISS-027 evidence: wasted ritual state JSONs archived
- examples/iss019_smoke_pilot.rs, examples/synthesis_bench.rs ported"
```

### Step 9: Deprecate the old repo

```bash
cd /Users/potato/clawd/projects/engram-ai-rust

# Prominent marker
cat > DEPRECATED.md <<'EOF'
# ⚠️ DEPRECATED 2026-04-22

Moved to: /Users/potato/clawd/projects/engram/ (monorepo)
GitHub:   https://github.com/tonitangpotato/engram-ai

All post-consolidation changes (commit 71f3654 + uncommitted ISS-024 WIP)
were ported on 2026-04-23. See ISS-025 in the monorepo.

DO NOT: commit, push, or run rituals here.
EOF

# Guard marker for ritual launcher (see ISS-027)
cat > .gid/DEPRECATED_DO_NOT_RITUAL <<'EOF'
Deprecated 2026-04-22. Canonical: /Users/potato/clawd/projects/engram/
EOF

# Rename remote to prevent accidental push
git remote rename origin deprecated-origin

# Stash the dirty state (everything was ported, but keep as evidence)
git add -A
git stash push -u -m "pre-deprecation snapshot (ported to monorepo ISS-025)"
```

## Acceptance Criteria

- [ ] Step 2: `git log` in monorepo shows a commit adapted from 71f3654 with rewritten paths
- [ ] Step 3: `src/dimension_access.rs` + `src/temporal_dim.rs` exist in monorepo `crates/engramai/src/`; patch file exists in `.gid/issues/ISS-024-.../wip/`
- [ ] Step 4: ISS-024 dir + ISS-019 VERIFY_REPORT + design snapshot exist in monorepo
- [ ] Step 5: 2 example files copied, cargo still builds
- [ ] Step 6: 3 ritual JSONs archived with README in `.gid/archive/iss-027-ritual-bug-evidence/`
- [ ] Step 7: `cargo test -p engramai` passes (≥1176 tests per 71f3654 baseline)
- [ ] Step 8: Monorepo has commit covering ports (steps 3-6)
- [ ] Step 9: Deprecated repo has `DEPRECATED.md`, `.gid/DEPRECATED_DO_NOT_RITUAL`, remote renamed, dirty state stashed

## Risks

| Risk | Mitigation |
|---|---|
| Step 2 `git am` conflicts | If conflicts: abort, manually rewrite patch, try again. Code base shouldn't have diverged much since paths are the only difference. |
| Step 5 examples don't compile | Delete them; monorepo build is priority. Open follow-up issue to fix imports. |
| Step 7 tests regress | Step 2 is the only thing that touches code; `git reset --hard HEAD~1` reverts. |
| Someone commits to deprecated repo between now and Step 9 | Low risk in the next hour. Do Steps 1-9 in one session. |

## Why Not Do Each Step in a Separate Session?

Because the deprecated repo is still writable. Every hour it stays writable without a deprecation marker is an hour where another ritual or human edit could add to the backlog. Do it all at once.

## Follow-ups (separate issues)

- **ISS-027** — ritual launcher guard (marker file check, dirty-tree refusal). The port of lint commit in Step 2 is only a patch; the root cause (no guard) is separately fixed.
- **ISS-026** — superseded by this issue. The lint commit IS being ported here, so ISS-026's "rerun clippy" plan is moot. Close ISS-026 as wontfix.
