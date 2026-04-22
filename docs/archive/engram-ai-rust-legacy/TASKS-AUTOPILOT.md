# engramai v0.2.3 — Autopilot Task Plan

> Project path: `/Users/potato/clawd/projects/engram-ai-rust`
> All commands must be run with: `export PATH="$HOME/.cargo/bin:$PATH"`
> Tests: `cargo test --workspace` — expect 710+ pass, 0 fail
> Single branch: `main`

---

## P0: Commit, Publish, Fix Critical Perf Bug (serial)

- [x] TASK-01: Commit Memory Supersession feature — `git add src/types.rs src/storage.rs src/lib.rs src/main.rs src/memory.rs src/association/candidate.rs src/association/former.rs src/confidence.rs src/models/actr.rs src/promotion.rs tests/dedup_test.rs tests/embedding_protocol_v2.rs` then `git commit -m "feat(supersession): filter-based memory correction — superseded_by field, storage exclusion, CLI correct/correct-bulk"`. Verify: `cargo test --workspace` all pass.

- [x] TASK-02: Commit Lifecycle Phase 4-5 — `git add src/lifecycle.rs src/memory.rs` then `git commit -m "feat(lifecycle): Phase 4-5 — health checks, sleep cycle with per-phase timing, rebalance"`. Verify: `cargo test lifecycle` and `cargo test sleep` pass.

- [x] TASK-03: Commit Synthesis incremental staleness — run `git diff HEAD -- src/synthesis/` to see which synthesis files changed, then `git add src/synthesis/engine.rs src/synthesis/types.rs` (and any other changed synthesis files) then `git commit -m "feat(synthesis): incremental staleness check — Jaccard distance + quality_score delta thresholds, skip unchanged clusters"`. Verify: `cargo test synthesis` pass.

- [x] TASK-04: Commit GID graph update — `git add .gid/` then `git commit -m "chore: update GID task graph"`. This is a simple bookkeeping commit.

- [x] TASK-05: Publish v0.2.3 to crates.io — Pre-checks: (1) `cargo test --workspace` all pass, (2) `cargo clippy -- -D warnings` clean, (3) `cargo doc --no-deps` builds, (4) `cargo package --list` no secrets, (5) version in Cargo.toml is 0.2.3, (6) `git status` clean, (7) `git push origin main`. Then: `cargo publish`, then `git tag v0.2.3 && git push origin v0.2.3`. Verify: check crates.io page.

- [x] TASK-06: Fix storage.all() hot-loop perf bug in src/synthesis/engine.rs around line 296 — `storage.all()` is called inside a per-cluster loop making it O(C×N). Fix: hoist the load above the loop and build a HashMap index, OR add a `storage.get_by_ids(&[&str])` method for targeted SQL fetch. Also audit other `storage.all()` call sites in src/promotion.rs and src/memory.rs for similar issues. Verify: `cargo test --workspace` all pass.

## P1: Fill Synthesis TODOs + Fix Flaky Test (parallel)

- [x] TASK-07: Wire emotional modulation into synthesis — src/synthesis/engine.rs line 268, uncomment or implement `apply_emotional_modulation`. Calculate avg emotional valence per cluster, sort clusters by abs(avg_valence) descending. Add test with memories of different valence values. Verify: `cargo test synthesis` pass.

- [x] TASK-08: Persist cluster attempt history — src/synthesis/engine.rs line 307, replace `let cluster_changed = true` with actual check using IncrementalState. Add last_attempt_timestamp and attempt_count fields. Use Jaccard distance to detect membership changes. Verify: `cargo test synthesis` pass.

- [x] TASK-09: Compute pairwise similarity — src/synthesis/engine.rs line 309, replace `let all_pairs_similar = false` with actual cosine similarity computation on member embeddings. Threshold 0.95 default. Sample pairs for clusters >10 members. Verify: `cargo test synthesis` pass.

- [x] TASK-10: Implement auto-update actions — src/synthesis/engine.rs line 445, implement the AutoUpdate match arm: MergeDuplicates (use supersession API), StrengthenLinks (boost Hebbian weights), UpdateMetadata. Verify: `cargo test synthesis` pass.

- [x] TASK-11: Implement stale cluster counting in health() — src/memory.rs line 807, replace `stale_clusters: 0` with actual count by checking if >50% of cached cluster members have been deleted/superseded. Verify: `cargo test health` or `cargo test lifecycle` pass.

- [x] TASK-12: Fix flaky test_merge_from_env — src/compiler/config.rs line 301, env var test uses set_var/remove_var which is not thread-safe. Fix with serial_test crate or a static Mutex lock. Verify: `cargo test --workspace` passes with parallel threads.

## P2: Deeper Improvements

- [x] TASK-13: Audit Knowledge Compiler integration — review src/compiler/ module, check CLI commands (engram compile, engram import), run `cargo test compiler`, document gaps. Write findings to this file. **→ See TASK-13-KC-AUDIT.md**

- [x] TASK-14: Connect Somatic markers to decision loop — check for existing somatic types (`grep -rn "somatic\|SomaticMarker" src/`), implement emotional memory association boost/penalty in recall ranking. Add tests.

- [x] TASK-15: Meta-Cognition Loop — implement self-monitoring: track recall accuracy, synthesis quality over time, auto-adjust parameters (decay rate, promotion threshold) based on feedback signals. This is exploratory — start with metrics collection.
