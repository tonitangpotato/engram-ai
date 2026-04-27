#!/usr/bin/env bash
#
# scripts/rollback-from-backup.sh — engram v0.3 → v0.2 full rollback (§8.6).
#
# This is the executable form of the 5-step checklist in
# docs/migration-rollback.md. It mirrors design §8.6 verbatim and is the
# body of the CI rollback drill (§8.4 / `task:mig-test-compat-rollback`).
#
# It satisfies GOAL-4.7 ("manual rollback from backup is possible, documented,
# and testable").
#
# IDEMPOTENCY: re-running on a DB that is already at schema_version=2 is a
# no-op (Steps 2-4 are skipped after the precondition check).
#
# USAGE
#   scripts/rollback-from-backup.sh <db-path>
#
# OPTIONS (env vars)
#   ENGRAM_BACKUP_PATH    Override the backup path (default: <db>.pre-v03.bak).
#   ENGRAM_FORCE          If "1", skip the lsof check (CI / no-lsof hosts).
#   ENGRAM_SKIP_LSOF      Same as ENGRAM_FORCE=1 (kept for clarity in CI).
#   ENGRAM_QUIET          If "1", suppress informational output (errors still print).
#
# EXIT CODES
#   0   Success (rollback applied OR already-rolled-back no-op).
#   1   Usage error (missing/extra arguments, file not found).
#   2   Backup integrity check failed.
#   3   DB still has writers (lsof reported holders) and ENGRAM_FORCE not set.
#   4   Post-rollback verification failed (schema_version != 2 after restore).
#   5   sqlite3 CLI missing on PATH.
#
# This script does NOT stop services. The operator must do that before
# invoking it (Step 1 of the runbook). The CI drill invokes it on a
# transient sqlite file that no service is holding, which is why this is
# safe.

set -euo pipefail

readonly BACKUP_SUFFIX="pre-v03.bak"
readonly EXPECTED_SCHEMA_VERSION="2"

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------

log() {
    if [[ "${ENGRAM_QUIET:-0}" != "1" ]]; then
        printf '[rollback] %s\n' "$*"
    fi
}

err() {
    printf '[rollback][error] %s\n' "$*" >&2
}

require_sqlite3() {
    if ! command -v sqlite3 >/dev/null 2>&1; then
        err "sqlite3 not found on PATH (needed for integrity check + schema_version verify)"
        exit 5
    fi
}

usage() {
    cat <<EOF >&2
usage: $(basename "$0") <db-path>

Rolls an engram v0.3 SQLite database back to v0.2 by restoring the
pre-migration backup written by 'engramai migrate' Phase 1.

See docs/migration-rollback.md for the full 5-step procedure.
EOF
    exit 1
}

# ---------------------------------------------------------------------------
# arg parsing
# ---------------------------------------------------------------------------

if [[ $# -ne 1 ]]; then
    usage
fi

DB_PATH="$1"
BACKUP_PATH="${ENGRAM_BACKUP_PATH:-${DB_PATH}.${BACKUP_SUFFIX}}"

if [[ ! -f "$DB_PATH" ]]; then
    err "DB file not found: $DB_PATH"
    exit 1
fi

if [[ ! -f "$BACKUP_PATH" ]]; then
    err "Backup file not found: $BACKUP_PATH"
    err "(Expected location is <db>.${BACKUP_SUFFIX}; override with ENGRAM_BACKUP_PATH.)"
    err "If migration was run with --no-backup, manual rollback is not possible — see docs/migration-rollback.md § Mid-migration rollback."
    exit 1
fi

require_sqlite3

# ---------------------------------------------------------------------------
# Idempotency short-circuit: if DB is already at v0.2, just verify and exit 0.
# ---------------------------------------------------------------------------

current_version="$(sqlite3 "$DB_PATH" 'SELECT MAX(version) FROM schema_version;' 2>/dev/null || echo 'unknown')"

if [[ "$current_version" == "$EXPECTED_SCHEMA_VERSION" ]]; then
    log "DB already at schema_version=${EXPECTED_SCHEMA_VERSION}; nothing to do (idempotent no-op)."
    exit 0
fi

log "Current schema_version: ${current_version}"
log "Target schema_version:  ${EXPECTED_SCHEMA_VERSION}"
log "DB:     ${DB_PATH}"
log "Backup: ${BACKUP_PATH}"

# ---------------------------------------------------------------------------
# Step 1 (operator-side): verify no writers hold the DB.
# We do not stop services from here; we only refuse to overwrite a live DB.
# ---------------------------------------------------------------------------

if [[ "${ENGRAM_FORCE:-0}" != "1" && "${ENGRAM_SKIP_LSOF:-0}" != "1" ]]; then
    if command -v lsof >/dev/null 2>&1; then
        if lsof -- "$DB_PATH" >/dev/null 2>&1; then
            err "DB is held by another process (lsof '$DB_PATH' returned holders)."
            err "Stop the engramai service first (see docs/migration-rollback.md Step 1)."
            err "If you are SURE no live writer holds it, set ENGRAM_FORCE=1 to bypass."
            exit 3
        fi
    else
        log "lsof not found; skipping live-writer check (set ENGRAM_FORCE=1 to silence this)."
    fi
fi

# ---------------------------------------------------------------------------
# Step 2: move aside the migrated DB and any WAL/SHM sidecars.
# ---------------------------------------------------------------------------

log "Step 2: moving aside migrated DB → ${DB_PATH}.v03-failed"
mv -- "$DB_PATH" "${DB_PATH}.v03-failed"

for sidecar in "${DB_PATH}-wal" "${DB_PATH}-shm"; do
    if [[ -f "$sidecar" ]]; then
        mv -- "$sidecar" "${sidecar}.v03-failed"
    fi
done

# ---------------------------------------------------------------------------
# Step 3: integrity-check the backup, then copy it into place.
# ---------------------------------------------------------------------------

log "Step 3: integrity-checking backup..."
integrity="$(sqlite3 "$BACKUP_PATH" 'PRAGMA integrity_check;' 2>&1 || true)"

if [[ "$integrity" != "ok" ]]; then
    err "Backup integrity check failed:"
    err "$integrity"
    err "DB has been moved aside to ${DB_PATH}.v03-failed; restore it manually if needed."
    # Best-effort: put the moved-aside file back so the operator is not stuck.
    mv -- "${DB_PATH}.v03-failed" "$DB_PATH"
    for sidecar in "${DB_PATH}-wal" "${DB_PATH}-shm"; do
        if [[ -f "${sidecar}.v03-failed" ]]; then
            mv -- "${sidecar}.v03-failed" "$sidecar"
        fi
    done
    exit 2
fi

log "Step 3: copying backup into place..."
cp -- "$BACKUP_PATH" "$DB_PATH"

# ---------------------------------------------------------------------------
# Step 4: verify schema_version is 2 and memories table is present.
# ---------------------------------------------------------------------------

post_version="$(sqlite3 "$DB_PATH" 'SELECT MAX(version) FROM schema_version;')"

if [[ "$post_version" != "$EXPECTED_SCHEMA_VERSION" ]]; then
    err "Post-restore schema_version is ${post_version}, expected ${EXPECTED_SCHEMA_VERSION}."
    err "Backup at ${BACKUP_PATH} is NOT a pre-v0.3 backup — investigate before restarting any service."
    exit 4
fi

memories_count="$(sqlite3 "$DB_PATH" 'SELECT COUNT(*) FROM memories;' 2>/dev/null || echo 'error')"
log "Step 4: schema_version=${post_version}, memories=${memories_count}"

# ---------------------------------------------------------------------------
# Step 5 (operator-side): restart engramai on the v0.2.x binary.
# This script intentionally does NOT do that — service management belongs
# to the operator's init system.
# ---------------------------------------------------------------------------

log "Rollback complete."
log "Next step (operator): restart engramai on the v0.2.x binary (NOT v0.3.x)."
log "Failed migration artefact preserved at: ${DB_PATH}.v03-failed"

exit 0
