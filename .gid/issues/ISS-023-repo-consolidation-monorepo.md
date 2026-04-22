# ISS-023: Repo Consolidation — Migrate Local Development from `engram-ai-rust/` to `engram/` Monorepo

**Status:** open (organizational / tech debt)
**Severity:** medium — not user-visible, but causes config drift, confusion about "source of truth", and duplicate work.
**Feature:** repo-structure
**Related:** n/a (organizational, not a code feature)
**Blocked by:** nothing
**Blocks:** any downstream project that pins to a specific engram repo path (e.g. cogmembench config, RustClaw Cargo dependency, hermes-engram integration)

## TL;DR

We currently have **two parallel engram repositories** on disk and on GitHub:

| Repo | Path | GitHub | Role | Contains ISS-021/022? |
|---|---|---|---|---|
| `engram-ai-rust/` | `/Users/potato/clawd/projects/engram-ai-rust/` | `tonioyeme/engram` | **Actual local development** | ✅ Yes |
| `engram/` | `/Users/potato/clawd/projects/engram/` | `tonitangpotato/engram-ai` | Published monorepo (stale) | ❌ No (stopped at `WIP: EmpathyBus rename pre-dimensional-merge`) |

The monorepo was set up deliberately a few days ago (library/binary split, consolidating core + CLI + MCP), and pushed to GitHub as the "public" engram. But **all subsequent local development continued in the old flat `engram-ai-rust/` repo**, including:

- ISS-019 (dimensional metadata write gap)
- ISS-020 (KC dimensional awareness)
- ISS-021 (subdim extraction coverage fix)
- ISS-022 (Vec<String> schema tech debt)
- The `wip/dimensional-recall-20260422` branch + merge to main

Result: the monorepo on GitHub is **~2 weeks behind real development**, and downstream tools (`cogmembench` config, this RustClaw workspace notes) reference inconsistent paths.

## Root Cause & History

From recall:
- ~early April 2026: user created monorepo `engram/` as a library/binary split refactor (no logic change), consolidating `engram-ai-rust` + CLI + MCP. Pushed to GitHub `tonitangpotato/engram-ai`.
- ~mid April 2026: daily work resumed in old `engram-ai-rust/` out of habit / because the feature branches were there. Monorepo never got re-synced.
- 2026-04-22: discovered during LoCoMo benchmark re-run — `cogmembench/benchmarks/locomo/config.py` points `ENGRAM_BINARY` at `/Users/potato/clawd/projects/engram/target/release/engram`, which doesn't exist (target dir was cleaned). Forced the question: which repo is the real one?

The decision made in discussion (recall hit, not yet filed): **monorepo is the destination, old repo is legacy**. This issue files that plan.

## Scope

### In scope

1. **Inventory the drift.** List every commit in `engram-ai-rust/` main that is not in `engram/` main. Catalog by type: features (ISS-019, 020, 021), tests, docs, GID issue files.
2. **Choose consolidation strategy.** Two options:
   - **(a) Replay** — cherry-pick or re-apply each feature branch onto the monorepo, preserving history where possible.
   - **(b) Wholesale import** — treat `engram-ai-rust/` as the source of truth, overwrite the monorepo's Rust subtree with it, then re-apply the monorepo's library/binary split on top.
   Decide based on how much the monorepo's structure has diverged from flat layout.
3. **Execute the migration** on a `wip/monorepo-consolidation-20260422` branch in `engram/`.
4. **Update downstream references:**
   - `cogmembench/benchmarks/locomo/config.py` — `ENGRAM_BINARY` path + any workspace dir refs
   - `cogmembench/benchmarks/longmemeval/config.py` — same
   - RustClaw workspace `MEMORY.md` / `TOOLS.md` — update "Engram project path" notes
   - Any `Cargo.toml` in other projects that path-depends on engram
   - `hermes-engram` / `autoresearch-engram` if they pin a path
5. **Archive `engram-ai-rust/`** — either rename to `engram-ai-rust.archived/`, add a big `ARCHIVED.md`, or delete after confirming monorepo builds + tests pass.
6. **Verify by rebuilding benchmarks** from the monorepo binary. LoCoMo conv-26 should run against the consolidated binary with no behavioral change from the pre-consolidation binary.

### Out of scope

- Changing engram's feature set, API, or behavior during consolidation.
- Re-running full benchmark suite as part of this issue (a sanity pass on conv-26 is sufficient).
- Deciding the long-term monorepo structure (core vs cli vs mcp crates) — assume existing monorepo layout is correct.
- GitHub repo renames / visibility changes — can happen after migration is green.

## Affected Code / Files

### `engram-ai-rust/` (source, to be migrated OUT)
- Entire `src/` tree (including all ISS-019+ changes)
- `.gid/issues/ISS-00[1-9]-*`, `ISS-01[0-9]-*`, `ISS-02[0-3]-*` — issue docs
- `tests/`, `examples/`, benchmarks under `benches/`
- Branches: `main`, `wip/dimensional-recall-20260422` (already merged), any other open feature branches

### `engram/` (destination, to receive migration)
- Keep current monorepo structure (crate split)
- Replace Rust source under core crate with `engram-ai-rust/src/`
- Merge `.gid/issues/` (engram currently only has ISS-001-synthesis-perf + reviews)

### Downstream (needs config update)
- `/Users/potato/clawd/projects/cogmembench/benchmarks/locomo/config.py` — line: `ENGRAM_BINARY = Path("/Users/potato/clawd/projects/engram/target/release/engram")` — already points at `engram/`, just needs the binary to actually exist there after consolidation.
- `/Users/potato/clawd/projects/cogmembench/benchmarks/longmemeval/config.py` — same pattern
- `/Users/potato/rustclaw/MEMORY.md`, `TOOLS.md` — path notes referencing engram

## Acceptance Criteria

- [ ] Drift inventory exists as a file in this issue dir: `drift-inventory.md` listing commits + feature areas present in `engram-ai-rust/` main but missing from `engram/` main.
- [ ] Migration strategy (replay vs wholesale) decided and documented in a short `strategy.md` in this issue dir.
- [ ] `engram/` main (or a `consolidation` branch ready to merge) contains all features currently in `engram-ai-rust/` main, including ISS-019 through ISS-022.
- [ ] `cargo test --all` passes in `engram/` monorepo (current count ≥ 961 lib tests).
- [ ] `cargo build --release --bin engram` in `engram/` produces a binary that can be pointed at by `cogmembench` config.
- [ ] Smoke test: LoCoMo conv-26 runs on the monorepo-built binary and completes without error (accuracy comparison not required here — that's ISS-021 / benchmark territory).
- [ ] Downstream config references updated (cogmembench, RustClaw notes).
- [ ] `engram-ai-rust/` archived: renamed or tagged with `ARCHIVED.md` explaining it's legacy, pointing to monorepo.
- [ ] GitHub `tonioyeme/engram` repo has a final commit noting archival + pointing to `tonitangpotato/engram-ai`, or is made private/read-only.

## Risk & Complexity

**Medium risk.** The code itself doesn't change — it's a structural move. But:
- Easy to miss a feature branch mid-migration → partial regression.
- Downstream projects may break silently if path updates are missed (the cogmembench case is exactly this failure mode, caught by benchmark failure).
- Git history will be messy; decide early whether to preserve commits 1:1 (replay) or squash into one "import" commit (wholesale).

**Complexity:** low-medium code effort, medium coordination (multiple downstream projects).

**Estimated effort:** 1–2 focused sessions. Drift inventory = 1 hour, strategy decision = 30 min, execute = 2–4 hours, downstream updates = 1 hour, verification = 1 hour.

## Notes

- **Do not start the migration until current in-flight work on `engram-ai-rust/` is checkpointed.** Specifically, let ISS-021 re-benchmark (LoCoMo conv-26 re-run) complete first so we have a clean baseline from the pre-consolidation world.
- After migration, `cogmembench` config is correct by construction — no path change needed.
- Consider whether to keep the RustClaw workspace's `engram-memory.db` path stable (`/Users/potato/rustclaw/engram-memory.db`) independent of repo location. It should not be affected since DB is at runtime path, not repo path.
