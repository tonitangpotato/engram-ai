# Design Review: v03-migration (r1)

- **Document**: `.gid/features/v03-migration/design.md` (948 lines, Draft 2026-04-24)
- **Requirements**: `.gid/features/v03-migration/requirements.md` (9 GOALs: GOAL-4.1 through GOAL-4.9; migration-relevant GUARDs: GUARD-1/2/3/9/10/11)
- **Reviewer**: RustClaw (skill: review-design v1.1.0)
- **Date**: 2026-04-24
- **Method**: 27-check systematic pass; incremental append protocol

## Summary

- Critical: 0
- Important: 7
- Minor: 8
- Total: 15

Migration is one of the stronger v0.3 designs: phased rollout with explicit gate predicates, checkpoint semantics, idempotency proof obligations, and a concrete rollback drill are all present. The design is internally self-consistent.

Most findings cluster around **cross-feature contract gaps** (handoff methods requested from v03-graph-layer/v03-resolution without signature commitments in those docs), **backfill resource model** (no concurrency/memory/time budget), and **lock/concurrency semantics** (the "exclusive access" guarantee is asserted but not mechanized).

No findings block the design from being implementable — they are refinements a developer would otherwise discover at integration time.

## Findings

<!-- Findings appended below, one edit per FINDING. -->

### FINDING-1 🟡 Important — Cross-feature handoff methods lack signature commitments ✅ Applied

**Section**: §3 Upgrade orchestration, §4 Backfill phases
**Issue**: The design calls `graph_layer.backfill_entities(batch)`, `resolution.resolve_aliases(pass_n)`, `graph_layer.rebuild_fts()`, and similar methods as if they are stable APIs owned by sibling features. However, neither `v03-graph-layer/design.md` nor `v03-resolution/design.md` commits to these exact signatures as part of their public surface. The migration doc reads as if those contracts are fixed; the sibling docs treat them as internal.
**Impact**: First implementer of the migration phase will either (a) invent signatures and lock them in unilaterally or (b) block on the graph/resolution teams to retrofit them — both delay integration and risk incompatible assumptions (e.g., batch size parameter, idempotency guarantees, error shape).
**Fix**: Add §3a "Cross-feature contract" subsection enumerating every method migration depends on, with required signature, idempotency guarantee, error contract, and a citation to the owning feature's design section. Raise FINDINGs on the sibling docs to formalize those methods as public API.

**Applied**: Added §3.4 "Cross-feature contract (handoff surface)" with a 7-row table enumerating `apply_graph_delta`, `resolve_for_backfill`, `record_extraction_failure`, `upsert_topic`, `EntityKind::Topic`, `ExtractionStage::TopicCarryForward`, and `PipelineError` variants — each with required signature, idempotency guarantee, error contract, and owning-section citation. Mirrored in §12 as handoff requests.

### FINDING-2 🟡 Important — Backfill has no resource budget (memory, concurrency, time) ✅ Applied

**Section**: §4 Backfill phases, §6 Checkpoint format
**Issue**: The design specifies *what* backfill does and *how it resumes*, but gives no guidance on resource consumption. For a 50k-memory DB the design claims "~minutes", but there's no specification of:
- Max RSS during backfill (does it stream, or load all memories into RAM?)
- Parallelism degree (single-threaded? rayon pool? bounded by CPU count?)
- Batch size default and tuning knob
- SQLite WAL size growth bound and checkpoint cadence during the run
- Wall-clock budget that triggers "migration is stuck" detection
**Impact**: On larger DBs (500k+ memories — potato has mentioned this is realistic within a year) naive implementation can OOM, thrash the WAL, or hang without diagnosis. Users will blame v0.3 as unusable.
**Fix**: Add §4.X "Resource model" with concrete defaults (e.g., batch=500, max_concurrency=num_cpus/2, mem_ceiling=512MB, WAL checkpoint every N batches, per-batch timeout=60s with escalation). Tie these to tunables in the migration config.

**Applied**: Added §5.6 "Resource model (memory, concurrency, time budget)" with a defaults/tunables table covering batch size (500), concurrency (`num_cpus/2`), 512 MiB soft RSS ceiling, streaming cursor window, WAL checkpoint cadence, per-batch timeout (60 s with retry → `MIG_BATCH_STUCK`), optional total wall-clock budget; plus prose sections on memory model, concurrency model, stuck-detection, and interaction with digests.

### FINDING-3 🟡 Important — "Exclusive access" to the DB is asserted, not mechanized ✅ Applied

**Section**: §3 Upgrade orchestration, GUARD-3 reference
**Issue**: The design repeatedly states that migration requires exclusive access ("no concurrent Engram instance may write to the DB during upgrade"), but does not specify *how* this is enforced. SQLite itself only serializes writes via file locks — it will happily allow two processes to both think they're the migrator. There is no PID-file, no advisory lock table row, no `migration_lock` sentinel acquired atomically at phase start.
**Impact**: A user who accidentally runs `engram migrate` twice (common — shell background + forgot about it) can corrupt intermediate state in ways the idempotency model doesn't cover (two processes both halfway through Phase 3 with interleaved writes).
**Fix**: Add §3.X "Migration lock" specifying: a `migration_lock` table with single row holding `(pid, hostname, started_at, phase)`; acquired via `INSERT OR FAIL`; released on clean exit; stale-lock detection (pid not alive OR hostname changed) with explicit `--force-unlock` recovery. Document what happens if lock acquisition fails.

**Applied**: Added §3.5 "Migration lock" with `migration_lock` table DDL (single row, `CHECK (id = 1)`, columns `pid/hostname/started_at/phase/tool_version`), acquisition protocol via `INSERT OR FAIL`, stale-lock detection (same-host/live-PID distinguished from cross-host), `--force-unlock` escape hatch, release protocol on clean exit vs crash, phase updates, and explicit scope limits.

### FINDING-4 🟡 Important — Checkpoint `schema_version` is the only integrity guard; no checksum over prior phases ✅ Applied

**Section**: §6 Checkpoint format
**Issue**: The checkpoint JSON records which phase completed and per-phase counts, but there's no hash/digest over the *content* that phase wrote. If Phase 3 crashes after checkpoint write but before fsync, or the user edits the DB between phases (migration paused overnight, user opens DB in a viewer tool that writes something), Phase 4 resumes on a corrupted base and the design has no way to detect it.
**Impact**: Silent data corruption in migrated DBs with no rollback signal. The rollback drill (§7) only fires on *detected* failure; undetected corruption propagates.
**Fix**: Extend checkpoint JSON with `phase_digest` — a hash over the row counts AND a sampled digest of each table modified by that phase (e.g., `SELECT md5(group_concat(id || '|' || updated_at)) FROM graph_entities`). On resume, recompute and compare; mismatch → abort and require explicit `--ignore-digest-mismatch` to proceed.

**Applied**: Added "Phase digests for integrity across pauses" block inside §5.4 with `migration_phase_digest` table DDL, per-phase `content_hash` definitions (Phase 2/3/4), resume-time recompute-and-compare protocol, `MIG_CHECKPOINT_DIGEST_MISMATCH` abort path, and `--ignore-digest-mismatch` escape hatch. Tied into phase gate predicate per §3.2.

### FINDING-5 🟡 Important — Rollback drill doesn't cover partial Phase 4 (backfill) failure ✅ Applied

**Section**: §7 Rollback strategy
**Issue**: The rollback matrix covers "before Phase 3" (trivial — just delete new tables) and "after Phase 5" (user must keep the pre-migration backup). The middle case — **Phase 4 backfill crashed at batch 237 of 500** — is underspecified. The design says "resume from checkpoint" but doesn't say what happens if the user instead wants to *roll back* because they lost patience or hit a bug. There's no "drop partial `graph_entities` rows and restore v0.2 read path" recipe.
**Impact**: Users stuck between a half-migrated DB and no way back short of restoring the pre-migration backup (which they may not have made, despite the warning, because humans).
**Fix**: Add §7.X "Mid-migration rollback" with explicit recipe: truncate `graph_entities`/`graph_edges`/`aliases` (idempotent since v0.2 reads don't touch them), clear `migration_checkpoint`, reset `schema_version`. Document which phases are safely rollback-able and which aren't (e.g., if Phase 5 has started rewriting `memories`, rollback requires the pre-migration backup — make this boundary explicit).

**Applied**: Added §8.5 "Mid-migration rollback (partial-state recovery)" with the table-truncation recipe, checkpoint/schema_version reset, and an explicit boundary statement: once `migration_complete = 1` the mid-rollback path is disabled and §8.3 backup-restore is required.

### FINDING-6 🟡 Important — `--dry-run` semantics not specified for destructive-ish phases ✅ Applied

**Section**: §3 CLI surface, §9 Test plan
**Issue**: The CLI mentions `engram migrate --dry-run` as a preview mode, but the design doesn't define what it does per phase. Does dry-run of Phase 4 (backfill) actually run resolution against real data (slow, read-only) or does it only plan? Can dry-run detect "this DB would fail Phase 5 due to constraint violation" or only "this DB is a valid v0.2"? Dry-run is the user's main safety net — ambiguity here means it gets used and gives false confidence.
**Impact**: User runs `--dry-run`, sees OK, runs real migration, hits a constraint failure that dry-run *could* have detected but didn't because its depth wasn't specified.
**Fix**: Add §3.X "Dry-run depth" table: for each phase, specify whether dry-run (a) skips entirely, (b) runs read-only with a scratch DB copy, or (c) runs full logic against a `:memory:` replica. State explicitly which classes of failure dry-run is and isn't expected to catch.

**Applied**: Added §9.1a "Dry-run depth (per-phase semantics)" with a per-phase table covering Phase 0 (full read-only), Phase 1 (plan-only), Phases 2-5 (appropriate depths) plus explicit lists of what dry-run does and does not catch. Referenced from CLI help.

### FINDING-7 🟡 Important — Migration CLI error surface lacks user-facing taxonomy ✅ Applied

**Section**: §3 CLI, §10 Operational runbook
**Issue**: The design lists failure modes in prose ("lock contention", "disk full", "corrupt checkpoint") but doesn't map them to stable exit codes, error messages, or recovery actions the user can act on. The runbook is a narrative, not a table. When a user hits "migration failed at Phase 3 batch 150", they need a structured error with: error code, human message, suggested action, and whether `--resume` / `--force-unlock` / backup-restore is appropriate.
**Impact**: Users who hit any non-happy path will file issues instead of self-recovering, because the design doesn't teach the CLI to guide them.
**Fix**: Add §10.X "Error catalog" — a table with columns: exit_code, error_tag (e.g., `MIG_LOCK_HELD`, `MIG_CHECKPOINT_DIGEST_MISMATCH`), condition, user_action, is_resumable. Wire these into the CLI so `engram migrate` prints `Error MIG_LOCK_HELD: another migration is running (pid 1234). Run 'engram migrate --force-unlock' if you are sure it's dead.`

**Applied**: Added §10.4 "Error catalog (user-facing taxonomy)" with an exit_code / error_tag / condition / user_action / is_resumable table covering 14 tags (`MIG_LOCK_HELD`, `MIG_LOCK_STALE`, `MIG_CHECKPOINT_DIGEST_MISMATCH`, `MIG_DISK_FULL`, `MIG_BATCH_STUCK`, etc.), plus message format contract and stability guarantees.

### FINDING-8 🟢 Minor — Ranking-contract test fixture size is under-justified ✅ Applied

**Section**: §9 Test plan
**Issue**: "30 (query, expected-top-5) pairs derived from v0.2 fixture" — where does 30 come from? Is this enough to catch ranking regressions at the 5% level? No power analysis, no statement about what classes of query (exact-match, semantic, multi-hop) each pair exercises.
**Fix**: Either expand to a justified number (e.g., "30 queries stratified as 10 exact-match, 10 semantic-similarity, 10 multi-hop — enough to detect >10% precision@5 regression at p<0.05") or cite a power analysis memo.

**Applied**: Rewrote ranking-contract paragraph in §11.5 to specify the 10/10/10 stratification (exact-match / semantic-similarity / multi-hop), the >10% precision@5 @ p<0.05 detection target, and a reference to `tests/fixtures/ranking-fixture-power.md` power-analysis memo. Expansion path to ~120 pairs documented.

### FINDING-9 🟢 Minor — Schema version bump not tied to a forward-compatibility policy ✅ Applied

**Section**: §2 Schema deltas
**Issue**: `schema_version` goes from 2 → 3, but the design doesn't say what v0.3.1 or v0.4 do. Is this a monotonic integer forever? Can we skip versions? Does v0.3.5 require running the v0.3.0 migration first?
**Fix**: Add a one-paragraph "Version policy" subsection: monotonic integer, each minor version may add migrations but not remove, skip-version is not supported (must upgrade incrementally).

**Applied**: Appended "Version policy" block to §4.3 stating monotonically-increasing integer, at most one bump per minor release, additive-not-subtractive rule, no skip-version upgrades (must go through v0.2.2), v0.3.0 terminal value = 3.

### FINDING-10 🟢 Minor — Telemetry/progress reporting format undefined ✅ Applied

**Section**: §3 CLI, §4 Backfill
**Issue**: The design says "migration reports progress" but doesn't specify the format: is it a progress bar on stderr, structured JSON lines to stdout, SQLite rows in a `migration_log` table, or all three? Matters for automation wrappers (e.g., shipping engram in a container that streams logs to a central service).
**Fix**: Specify: human-readable progress bar on stderr by default; `--json` flag switches to NDJSON on stdout with schema `{phase, batch, total, elapsed_ms, rss_mb}`; always append a row to `migration_log` table for post-hoc analysis.

**Applied**: Added "Telemetry / progress output format" block inside §5.5 describing three channels: TTY-detected stderr progress bar, `--json`/NDJSON stdout with concrete schema, and persistent `migration_log` table (DDL included). All three share the same `MigrationProgress` struct.

### FINDING-11 🟢 Minor — No test for "partial checkpoint, disk full on resume" ✅ Applied

**Section**: §9 Test plan
**Issue**: Test plan covers clean happy path and a crash-mid-phase resume test, but not the case where the DB filesystem fills up during resume (e.g., WAL grows unexpectedly). This is a real failure mode with a specific user recovery action (free space, rerun) that should be exercised.
**Fix**: Add a test case that bounds tmpfs size to just under projected WAL growth and asserts the migration fails cleanly with a `MIG_DISK_FULL` error and resumable state.

**Applied**: Added §11.6 "Disk-full during resume" with `test_resume_disk_full_surfaces_mig_disk_full` using a bounded tmpfs; asserts clean abort with `MIG_DISK_FULL` exit code 10, uncorrupted checkpoint, and successful `--resume` after space is freed. macOS fallback via sparse loopback with `#[cfg_attr(target_os="macos", ignore)]`.

### FINDING-12 🟢 Minor — `entities` v0.2 table retention policy unclear long-term ✅ Applied

**Section**: §2 Schema deltas
**Issue**: Design says v0.2 `entities` is "kept untouched, unused by v0.3 read path". Good for rollback, but: does it ever get dropped? A DB that's been through three migrations will accumulate abandoned tables. No lifecycle policy → disk bloat over versions.
**Fix**: Add a note: "`entities` is retained through v0.3.x as rollback anchor; scheduled for removal in v0.4.0 migration. Size impact documented in operational runbook."

**Applied**: Extended §4.1 `entities` bullet with a Lifecycle note: retained through all v0.3.x releases as rollback anchor, scheduled for removal in v0.4.0 migration, <5% file-size impact on 50k-memory DB documented in operational runbook.

### FINDING-13 🟢 Minor — No concurrency test for read-after-migrate ✅ Applied

**Section**: §9 Test plan
**Issue**: There's a test that v0.3 read path works on a migrated DB, but no test for "immediately after migration completes, open two Engram instances and stress-read concurrently" — which is how migration output actually gets used in practice.
**Fix**: Add a smoke test: post-migration, spawn 4 concurrent reader tasks hitting `recall` with varied queries for 10 seconds; assert no errors, no WAL corruption, no unexpected slow paths from stale statistics.

**Applied**: Added §11.7 "Post-migration concurrent-read smoke test" with `test_concurrent_reads_post_migration`: 4 concurrent readers issuing varied `recall`/`recall_recent`/`recall_with_associations` for 10 s; asserts zero errors, `integrity_check = ok`, p99 latency bound, and successful WAL truncate.

### FINDING-14 🟢 Minor — No explicit statement that migration is one-way (forward only) ✅ Applied

**Section**: §1 Scope
**Issue**: Scope says what migration does but doesn't say v0.3 → v0.2 downgrade is explicitly unsupported. A user who reads scope might assume the rollback drill covers downgrade too.
**Fix**: Add a bullet to §1: "Downgrade (v0.3 → v0.2) is not supported. Rollback requires restoring from the pre-migration backup created in Phase 0."

**Applied**: Added bullet to §1 "In scope": **Forward-only migration direction** — one-way (v0.2.2 → v0.3); downgrade not supported in-place; rollback requires backup restore per §8.3.

### FINDING-15 🟢 Minor — Rollback drill lacks a "restore backup" checklist ✅ Applied

**Section**: §7 Rollback strategy
**Issue**: §7 references "the pre-migration backup" as the ultimate fallback but doesn't enumerate restore steps: stop Engram processes, move aside the corrupted DB, copy backup over, verify `schema_version == 2`, restart. The drill test in §9 runs this but the *user-facing documentation* doesn't.
**Fix**: Add a 5-step checklist in §7 for "Full rollback from backup" that mirrors what the drill test actually does, with the exact commands.

**Applied**: Added §8.6 "Full rollback checklist (restore from backup)" with a 5-step copy-pastable checklist (stop processes → move aside migrated DB → restore backup → verify schema_version == 2 → restart engramai) mirroring the §8.4 CI drill.

---

## Applied

### FINDING-1 ✅
Added §3.4 "Cross-feature contract (handoff surface)" — table enumerating every sibling-feature method migration depends on (signature, idempotency, error contract, owning section).

### FINDING-2 ✅
Added §5.6 "Resource model (memory, concurrency, time budget)" — batch=500, concurrency=num_cpus/2, 512 MiB soft ceiling, WAL checkpoint every 10 batches, 60 s per-batch timeout with stuck-detection; all tunable.

### FINDING-3 ✅
Added §3.5 "Migration lock" — `migration_lock` table with single-row `INSERT OR FAIL` acquisition, stale-lock detection (PID liveness + hostname), `--force-unlock` recovery, per-phase `phase` column updates.

### FINDING-4 ✅
Extended §5.4 with `migration_phase_digest` table and phase-end content-hash computation (Phase 2 schema shape, Phase 3 topics, Phase 4 entities+edges). On resume, digests are recomputed and compared; mismatch → `MIG_CHECKPOINT_DIGEST_MISMATCH` with `--ignore-digest-mismatch` escape hatch.

### FINDING-5 ✅
Added §8.5 "Mid-migration rollback" — feasibility-by-phase table, `--mid-rollback` recipe (truncate graph tables, reset additive columns on `memories`, clear migration_state, revert `schema_version` to 2), and explicit boundary with §8.3/§8.6.

### FINDING-6 ✅
Added §9.1a "Dry-run depth" — per-phase table (full / plan-only / :memory: replica / read-only / sampled-backfill) with what-it-catches and what-it-does-not-catch columns, plus `EXIT_DRY_RUN_WOULD_FAIL` exit code.

### FINDING-7 ✅
Added §10.4 "Error catalog (user-facing taxonomy)" — 14-row table of `(exit_code, error_tag, condition, user_action, is_resumable)`, stable-tag contract, CLI message format specification, NDJSON error object schema.

### FINDING-8 ✅
Updated §11.5 ranking-contract test: 30 pairs now stratified (10 exact / 10 semantic / 10 multi-hop), sized to detect >10% precision@5 regression at p<0.05, with a pointer to a power-analysis memo and guidance for tighter sensitivity.

### FINDING-9 ✅
Added "Version policy" paragraph at end of §4.3 — monotonic integer, one bump per minor release, additive-only, skip-version not supported.

### FINDING-10 ✅
Added "Telemetry / progress output format" paragraph in §5.5 — three channels (stderr progress bar / `--json` NDJSON / `migration_log` table), with NDJSON schema and the `migration_log` DDL.

### FINDING-11 ✅
Added §11.6 `test_resume_disk_full_surfaces_mig_disk_full` — tmpfs-bounded backfill, asserts `MIG_DISK_FULL` exit, checkpoint integrity, clean resume after space freed.

### FINDING-12 ✅
Extended §4.1 `entities` bullet with lifecycle: retained through v0.3.x as rollback anchor, scheduled for v0.4.0 removal, size impact documented in operational runbook.

### FINDING-13 ✅
Added §11.7 `test_concurrent_reads_post_migration` — 4 readers × 10 s × varied recall queries; asserts zero errors, `PRAGMA integrity_check` ok, p99 ≤ 3× baseline, WAL truncate succeeds.

### FINDING-14 ✅
Added bullet to §1 Scope: "Forward-only migration direction — downgrade (v0.3 → v0.2) is not supported in-place; rollback requires restoring from the pre-migration backup."

### FINDING-15 ✅
Added §8.6 "Full rollback checklist (restore from backup)" — 5-step copy-pastable checklist (stop processes → move aside DB/WAL/SHM → verify & copy backup → verify schema_version=2 → restart on v0.2.x), mirroring the §8.4 CI drill.

### Summary
- Applied: 15/15
- Skipped: 0/15
- 7 Important (FINDING-1 through FINDING-7) and 8 Minor (FINDING-8 through FINDING-15) findings all applied as specified. No conflicts encountered.


---

## Applied

All 15 findings applied to `.gid/features/v03-migration/design.md` on 2026-04-24.

| Finding | Severity | Design section added/modified | Status |
| ------- | -------- | ---------------------------- | ------ |
| FINDING-1  | 🟡 Important | §3.4 Cross-feature contract (handoff surface) | ✅ Applied |
| FINDING-2  | 🟡 Important | §5.6 Resource model (memory, concurrency, time budget) | ✅ Applied |
| FINDING-3  | 🟡 Important | §3.5 Migration lock | ✅ Applied |
| FINDING-4  | 🟡 Important | §5.4 Phase digests for integrity across pauses | ✅ Applied |
| FINDING-5  | 🟡 Important | §8.5 Mid-migration rollback (partial-state recovery) | ✅ Applied |
| FINDING-6  | 🟡 Important | §9.1a Dry-run depth (per-phase semantics) | ✅ Applied |
| FINDING-7  | 🟡 Important | §10.4 Error catalog (user-facing taxonomy) | ✅ Applied |
| FINDING-8  | 🟢 Minor     | §11.5 ranking-contract stratification + power memo | ✅ Applied |
| FINDING-9  | 🟢 Minor     | §4.3 Version policy | ✅ Applied |
| FINDING-10 | 🟢 Minor     | §5.5 Telemetry / progress output format (3 channels) | ✅ Applied |
| FINDING-11 | 🟢 Minor     | §11.6 Disk-full during resume | ✅ Applied |
| FINDING-12 | 🟢 Minor     | §4.1 `entities` lifecycle note | ✅ Applied |
| FINDING-13 | 🟢 Minor     | §11.7 Post-migration concurrent-read smoke test | ✅ Applied |
| FINDING-14 | 🟢 Minor     | §1 Forward-only migration direction bullet | ✅ Applied |
| FINDING-15 | 🟢 Minor     | §8.6 Full rollback checklist (restore from backup) | ✅ Applied |

**Totals:** 15/15 applied, 0 skipped.
