# Engram v0.3 Migration — Rollback Runbook

> **Audience**: operators rolling an engramai database back from v0.3 to v0.2.
> **Source of truth**: design `.gid/features/v03-migration/design.md` §8 (Rollback & Pre-Migration Safety).
> **Requirement satisfied**: GOAL-4.7 (manual rollback from backup is documented + testable).
> **CI drill**: this runbook is the body of the rollback drill in §8.4 / §11.4-§11.5; the script `scripts/rollback-from-backup.sh` is the executable form of the §8.6 checklist.

---

## When to use which procedure

Engram v0.3's migration is **forward-only at the schema level**. There is no `engramai migrate --rollback` command (and there will never be — see [§ Why no in-tool rollback?](#why-no-in-tool-rollback)). Rolling back means one of two things, depending on how far the migration progressed:

| Situation                                                                       | Procedure                                                                    |
| ------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| Migration **completed Phase 5** (`migration_complete = 1` in `migration_state`) | **Full rollback from backup** — see [§ Full rollback (5-step checklist)](#full-rollback-5-step-checklist). |
| Migration **interrupted/crashed mid-run** (Phase 0–4)                            | **Mid-migration rollback** — see [§ Mid-migration rollback](#mid-migration-rollback). |
| You only want a clean retry after a Phase 0/1 failure                            | Just rerun `engramai migrate`. Phase 0–1 leave the source DB untouched.       |

If unsure: **prefer full rollback from backup.** It is unconditionally safe and is the path the CI drill exercises.

---

## Full rollback (5-step checklist)

This mirrors design §8.6 exactly. The script `scripts/rollback-from-backup.sh` is the executable form — running the script is equivalent to running these steps, and is what the §8.4 CI drill invokes.

> **Prerequisites:**
> - You have the backup file `<db>.pre-v03.bak` (written by Phase 1 of `engramai migrate` unless you opted out with `--no-backup`).
> - You have a v0.2.x engramai binary available to restart the service against.
> - You are operating on the live DB host (the script does **not** know how to reach a remote DB).

### Step 1 — Stop all engramai processes holding the DB

```bash
pkill -f engramai           # or: systemctl stop <your-service>
lsof /var/lib/engramai/data.db    # should print nothing
```

If `lsof` shows a process holding the file, do not proceed. SQLite cannot be safely swapped under a live writer.

### Step 2 — Move aside the migrated (or partially-migrated) DB

```bash
mv /var/lib/engramai/data.db         /var/lib/engramai/data.db.v03-failed
mv /var/lib/engramai/data.db-wal     /var/lib/engramai/data.db-wal.v03-failed 2>/dev/null || true
mv /var/lib/engramai/data.db-shm     /var/lib/engramai/data.db-shm.v03-failed 2>/dev/null || true
```

The `-wal` / `-shm` files are SQLite WAL artifacts; they may not exist depending on journal mode. Moving them aside (rather than deleting) preserves them for post-mortem.

### Step 3 — Verify the backup and copy it into place

```bash
sqlite3 /var/lib/engramai/data.db.pre-v03.bak "PRAGMA integrity_check;"
# expect: ok

cp /var/lib/engramai/data.db.pre-v03.bak /var/lib/engramai/data.db
```

If `integrity_check` does not print exactly `ok`, **stop**. The backup is corrupt; restoring it will replace one broken state with another. Open an incident and consult the `.v03-failed` file you set aside in Step 2 — it may still contain recoverable data.

### Step 4 — Verify schema_version is back to 2

```bash
sqlite3 /var/lib/engramai/data.db "SELECT MAX(version) FROM schema_version;"
# expect: 2

sqlite3 /var/lib/engramai/data.db "SELECT COUNT(*) FROM memories;"
# expect: same count as pre-migration (compare against your monitoring snapshot)
```

If `schema_version` is anything other than 2, the file at `<db>.pre-v03.bak` was not the pre-v0.3 backup. **Stop and investigate** before restarting any service.

### Step 5 — Restart engramai on the v0.2.x binary

```bash
systemctl start <your-service>     # MUST be running the pre-upgrade v0.2.x binary
```

Verify normal recall works against the restored DB. A canonical health check is to issue one query that previously worked and confirm identical results.

> ⚠️ **Do not restart on a v0.3 binary.** A v0.3 binary opened against a `schema_version = 2` database will refuse to start (forward-only check) — that is the intended safety, but the operational outcome is "service down." Pin your deploy back to v0.2.x first.

---

## Mid-migration rollback

If migration was interrupted (crash, Ctrl-C, host OOM, disk full), the path forward depends on how far the phase machine got. This is design §8.5 verbatim, presented as a decision tree.

### What phase did we reach?

```bash
sqlite3 /var/lib/engramai/data.db \
  "SELECT phase, migration_complete FROM migration_state ORDER BY rowid DESC LIMIT 1;"
```

If the query errors with "no such table: migration_state", you never made it past Phase 0 — there is nothing to roll back. Just rerun `engramai migrate`.

### Decision table

| Phase reached      | Rollback path                                                             | Backup required?   |
| ------------------ | ------------------------------------------------------------------------- | ------------------ |
| Phase 0 (preflight) | No DB change. Drop `migration_lock` row if present and exit.              | No.                |
| Phase 1 (backup)    | Optionally delete `<db>.pre-v03.bak`; otherwise no DB change.             | No.                |
| Phase 2 (DDL)       | `engramai migrate --mid-rollback` (see recipe below).                     | No.                |
| Phase 3 (topics)    | Same recipe; also truncates `knowledge_topics`.                           | No.                |
| Phase 4 (backfill)  | Same recipe; also clears graph tables and the additive `memories` columns.| No.                |
| Phase 5 (verify)    | Same recipe (Phase 5 is read-only, so no extra cleanup).                  | No.                |
| Phase 5 **completed** (`migration_complete = 1`) | **Full rollback from backup required** — see [§ Full rollback](#full-rollback-5-step-checklist). | **Yes — backup needed.** |

### `engramai migrate --mid-rollback` recipe

The CLI applies the following transactionally (single `IMMEDIATE` transaction). Documented here so an operator can also run it by hand if the CLI is broken; in normal operations, **prefer the CLI** — it asserts the precondition.

1. Assert `migration_state.migration_complete == 0`. If 1, abort with `MIG_ROLLBACK_COMPLETED_MIGRATION` (exit code 14) — the operator must use [§ Full rollback](#full-rollback-5-step-checklist) instead.
2. Truncate the v0.3-only tables:
   ```sql
   DELETE FROM graph_entities;
   DELETE FROM graph_entity_aliases;
   DELETE FROM graph_edges;
   DELETE FROM graph_predicates;
   DELETE FROM graph_memory_entity_mentions;
   DELETE FROM graph_extraction_failures;
   DELETE FROM knowledge_topics;
   DELETE FROM episodes;
   DELETE FROM affect_mood_history;
   ```
3. Reset the additive columns on `memories`:
   ```sql
   UPDATE memories
      SET entity_ids = '[]',
          edge_ids = '[]',
          episode_id = NULL,
          confidence = NULL;
   ```
   The columns themselves remain (SQLite cannot drop them cleanly). v0.2 code does not read these columns, so their presence is invisible to the rolled-back binary.
4. Clear the migration scaffolding:
   ```sql
   DELETE FROM migration_state;
   DELETE FROM migration_phase_digest;
   DELETE FROM migration_lock;
   ```
5. `UPDATE schema_version SET version = 2 WHERE version = 3;` (and delete any stray `version = 3` row if the single-row schema was violated).
6. Commit.

Post-recipe, the DB is **structurally v0.3** (new tables and columns exist but are empty / default) and **behaviorally v0.2** (v0.3 read paths see no graph data; v0.2 read paths see the original `memories` and `hebbian_links` unchanged). The operator can:

- Re-run `engramai migrate` to retry, **or**
- Restore from the pre-migration backup using [§ Full rollback](#full-rollback-5-step-checklist) if they want the file to look pristine.

### When to choose mid-rollback vs full rollback

- Pick **mid-rollback** if you want to retry the migration soon (no backup churn, fastest path back to a runnable state).
- Pick **full rollback** if you intend to **stay** on v0.2 for a while or you suspect the partially-migrated DB is in an unsafe state. Full rollback restores byte-for-byte content equivalence with the pre-migration snapshot.

---

## Why no in-tool rollback?

Two design constraints, documented in design §8.3, make `engramai migrate --rollback` (full reverse) the wrong abstraction:

1. **Lock contention.** A full rollback needs to happen while the process that would run such a command is *not* holding the DB. A filesystem-level script (`mv` / `cp`) is the right tool; an in-tool command would have to bootstrap its own lock dance just to release the DB it needs to overwrite.
2. **DDL irreversibility.** SQLite cannot cleanly drop columns that contain data. An in-tool rollback would either leak v0.3-shape columns forever or implement a fragile table-rebuild dance. The backup-restore approach sidesteps the problem entirely — the backup *is* the v0.2-shape file.

`--mid-rollback` is in-tool only because the partial-state path does not need the binary to release its DB (it owns the lock for the duration of the recipe), and because Phase 2-4 leftovers are *deletions* / *resets*, not column drops.

---

## Idempotency

- **Full rollback** (§8.6 / `scripts/rollback-from-backup.sh`): re-running the script after a successful rollback is a no-op. Detection is `schema_version == 2` ⇒ skip Steps 2–4, just verify and exit.
- **Mid-rollback** (`engramai migrate --mid-rollback`): re-running the recipe once `schema_version == 2` and `migration_state` is empty is a no-op (each `DELETE` removes zero rows; `UPDATE schema_version` is a no-op when version is already 2).

This idempotency is what makes the CI drill (§8.4) safe to retry.

---

## CI drill (§8.4)

Every CI build runs the rollback drill end-to-end:

1. Build a v0.2 fixture database (per design §11.1).
2. Run `engramai migrate` through Phase 5.
3. Assert `schema_version == 3`, backfill counters correct, topic count correct.
4. Run `scripts/rollback-from-backup.sh` against the migrated DB.
5. Assert `schema_version == 2` and the rolled-back DB hash equals the original fixture hash (content hash, not file hash — SQLite page metadata is allowed to differ).
6. Open the rolled-back DB with a v0.2.2 reader harness and verify a canonical `recall` query returns the same result as on the pristine fixture.

Step 6 is the strongest assertion: it proves not just that the file *looks* identical, but that v0.2 code *reads* the rolled-back DB correctly. Any change that breaks this test blocks the merge.

The drill itself lives in the migration test crate — see `task:mig-test-compat-rollback` (T18) for the test code that wraps this script.

---

## See also

- Design `.gid/features/v03-migration/design.md` §8 (Rollback & Pre-Migration Safety).
- Design `.gid/features/v03-migration/design.md` §10 (Error catalog — `MIG_BACKUP_FAILED`, `MIG_ROLLBACK_COMPLETED_MIGRATION`).
- Requirements `.gid/features/v03-migration/requirements.md` GOAL-4.7.
- Script: `scripts/rollback-from-backup.sh` (executable form of [§ Full rollback](#full-rollback-5-step-checklist)).
