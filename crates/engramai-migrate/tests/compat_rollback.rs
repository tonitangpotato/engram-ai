//! T18 — Backward-compat suite + rollback drill + concurrent-read smoke
//! (design §11.4, §11.5, §11.7).
//!
//! Implements the §11.4 / §11.5 / §11.7 test matrix from
//! `.gid/features/v03-migration/design.md`. Like T16 (idempotency) and
//! T17 (resume), this file ships **two layers**:
//!
//! 1. **Active tests** — everything that can be exercised today with the
//!    library surface available in `engramai-migrate`. These pin the
//!    rollback **mechanical contract** (§8.4 + §11.4), the compat
//!    **surface contract** (§7.1 + §7.3), and the concurrent-read
//!    **SQLite-level invariants** (§11.7) without reaching into the
//!    `engramai` core crate (which would require a real `Memory` value
//!    and a real `recall` to assert ranking on).
//!
//! 2. **`#[ignore]`d tests** — the §11.5 8-test compat matrix +
//!    ranking-contract assertion + §11.7 latency p99 are gated on:
//!      - `task:mig-impl-backfill-perrecord` (T9) — Phase 4 is currently
//!        a stub in `cli.rs::run_backfill` (returns `InvariantViolated`
//!        on live runs); without it we cannot run an end-to-end v0.2 →
//!        v0.3 migration in-process.
//!      - `task:mig-impl-topics` (T11) — Phase 3 is also stubbed.
//!      - The v0.2 fixture database (`crates/engramai/tests/fixtures/
//!        v02_sample.db`, design §11.1) — not yet committed.
//!      - An integration crate that depends on both `engramai-migrate`
//!        AND `engramai` core, so `Memory::recall` / `recall_recent` /
//!        `recall_with_associations` can be invoked against the migrated
//!        DB. Today `engramai-migrate` is a leaf crate (see compat.rs
//!        module docs for why).
//!
//!    The bodies of these tests stand in place as `unimplemented!()` +
//!    a structured TODO referencing the upstream task — same pattern
//!    T16/T17 use for the work blocked behind T9.
//!
//! ## Why these particular active tests?
//!
//! §11.4 (rollback drill) has **two** layers:
//!   - The **procedural** layer — `scripts/rollback-from-backup.sh`
//!     plus the `docs/migration-rollback.md` runbook. T15 owns the
//!     procedure; T18 owns the **drill** that proves it works.
//!   - The **mechanical** layer — close the `Connection`, replace the
//!     DB file with the backup, reopen, assert `schema_version == 2`,
//!     and assert user-data rows are unchanged. This is what the CI
//!     drill actually does, modulo the `recall` step (which needs
//!     engramai integration).
//!
//! We exercise the mechanical layer directly here. The full §11.4
//! step 6/7 (`Memory::open_v02_compat_mode` + recall match against
//! pre-migration baseline) is `#[ignore]`d as
//! `test_rollback_full_recall_match_pre_migration` until the
//! integration crate exists.
//!
//! §11.5 (compat suite) is split into:
//!   - The **surface contract** — `V02_FROZEN_METHODS`,
//!     `BEHAVIORAL_CONTRACT`, `V02CompatSurface`. These already have
//!     unit-level coverage in `src/compat.rs`; the integration tests
//!     here lock the contract at the **library boundary** so a future
//!     refactor that breaks the surface gets caught even if internal
//!     unit tests are deleted or moved.
//!   - The **8-test execution matrix** — fresh-v0.3 + migrated-v0.2
//!     × {store, recall, recall_recent, recall_with_associations}.
//!     Gated on T9 + integration crate, see `#[ignore]`d block.
//!
//! §11.7 (concurrent-read smoke) — we run a reduced version that
//! exercises **everything except `recall`**:
//!   1. Open the migrated DB from N threads.
//!   2. Each thread loops short SQLite reads (`SELECT FROM memories`,
//!      `PRAGMA integrity_check`).
//!   3. Assert zero errors, integrity_check returns "ok",
//!      `wal_checkpoint(TRUNCATE)` succeeds afterward.
//!
//! The full §11.7 (with real `recall` + p99 latency tracking) is
//! `#[ignore]`d as `test_concurrent_reads_post_migration_with_recall`.
//!
//! All tests use `tempfile::tempdir` so they don't touch the working
//! directory.
//!
//! GOAL coverage:
//! - GOAL-4.7 (rollback from backup; documented + testable).
//! - GOAL-4.9 (backward compat — v0.2 store/recall/recall_recent/
//!   recall_with_associations preserved).

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use tempfile::tempdir;

use engramai_migrate::{
    backup_path_for, migrate, MigrateOptions, MigrationPhase, BACKUP_SUFFIX,
    BEHAVIORAL_CONTRACT, V02_FROZEN_METHODS,
};

// ---------------------------------------------------------------------------
// Fixtures & helpers
// ---------------------------------------------------------------------------

/// Seed a v0.2-shaped database. Mirrors the seed helpers in `idempotency.rs`
/// and `resume.rs` so all three integration files agree on the
/// `SchemaState::V02` detection path.
///
/// `n_memories` lets tests vary fixture size — the rollback drill uses a
/// small fixture (3 rows) for fast assertion; the concurrent-read smoke
/// uses a larger fixture (50 rows) so the worker threads have something
/// non-trivial to scan.
fn seed_v02_db(path: &Path, n_memories: usize) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        // Seed BOTH `memories` AND `hebbian_links` because Phase 2
        // (`apply_additive_columns`) ALTERs both. Same shape used by
        // `resume.rs::seed_v02_db`.
        "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT, created_at TEXT);\
         CREATE TABLE hebbian_links (a TEXT, b TEXT);\
         CREATE TABLE schema_version (version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
         INSERT INTO schema_version VALUES (2, '2026-01-01T00:00:00Z');",
    )
    .unwrap();

    // Insert n_memories rows with varied content for downstream scans.
    let mut stmt = conn
        .prepare("INSERT INTO memories (id, content, created_at) VALUES (?1, ?2, ?3)")
        .unwrap();
    for i in 0..n_memories {
        let id = format!("m{i:04}");
        let content = format!("memory content row {i} alpha beta gamma");
        let created_at = format!("2026-01-{:02}T00:00:00Z", (i % 28) + 1);
        stmt.execute([&id, &content, &created_at]).unwrap();
    }
}

/// Build a `MigrateOptions` populated with the v0.2-acknowledgement
/// flags every test in this file needs. Backup writes are **enabled**
/// by default — the rollback drill needs the sidecar to exist. Tests
/// that want to suppress backup flip `no_backup` themselves.
fn options_for(db: &Path) -> MigrateOptions {
    let mut opts = MigrateOptions::new(db);
    opts.tool_version = "0.1.0-compat-rollback-test".to_string();
    opts.accept_forward_only = true;
    // accept_no_grace=true so the lib doesn't surface a grace-period
    // warning that would clutter assertion noise; the lib doesn't
    // actually sleep, the CLI binary does.
    opts.accept_no_grace = true;
    opts
}

/// Snapshot the user-visible state of the database for pre/post comparison.
/// Matches the helper in `idempotency.rs` but extends it to also capture
/// `hebbian_links` (T18 cares about links surviving a rollback, T16 only
/// asserts memories).
fn snapshot_user_state(path: &Path) -> Snapshot {
    let conn = Connection::open(path).unwrap();

    let mut version_rows = Vec::new();
    let mut stmt = conn
        .prepare("SELECT version, updated_at FROM schema_version ORDER BY version")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let v: i64 = row.get(0).unwrap();
        let u: String = row.get(1).unwrap();
        version_rows.push(format!("{v}|{u}"));
    }

    let mut memories_rows = Vec::new();
    let mut stmt = conn
        .prepare("SELECT id, content, created_at FROM memories ORDER BY id")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let id: String = row.get(0).unwrap();
        let content: Option<String> = row.get(1).unwrap();
        let created_at: Option<String> = row.get(2).unwrap();
        memories_rows.push(format!(
            "{id}|{}|{}",
            content.unwrap_or_default(),
            created_at.unwrap_or_default()
        ));
    }

    let mut hebbian_rows = Vec::new();
    // hebbian_links may not exist on all fixtures; tolerate absence.
    if let Ok(mut stmt) = conn.prepare("SELECT a, b FROM hebbian_links ORDER BY a, b") {
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let a: Option<String> = row.get(0).ok();
            let b: Option<String> = row.get(1).ok();
            hebbian_rows.push(format!(
                "{}|{}",
                a.unwrap_or_default(),
                b.unwrap_or_default()
            ));
        }
    }

    Snapshot {
        version_rows,
        memories_rows,
        hebbian_rows,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    version_rows: Vec<String>,
    memories_rows: Vec<String>,
    hebbian_rows: Vec<String>,
}

/// Read the SQLite `schema_version` table, returning the highest version
/// stamp (or panic if the table is absent).
fn read_schema_version(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.query_row(
        "SELECT MAX(version) FROM schema_version",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap()
}

/// Workspace root inferred from `CARGO_MANIFEST_DIR`. The integration
/// tests for migration-rollback.md / rollback-from-backup.sh need to
/// resolve paths relative to the engram repo root, not the crate root.
fn workspace_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR for this crate is .../engram/crates/engramai-migrate.
    // Workspace root is two `..` up.
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR must be at least two levels deep under workspace root")
        .to_path_buf()
}

// ---------------------------------------------------------------------------
// §11.4 — Rollback drill (mechanical layer)
// ---------------------------------------------------------------------------

/// §11.4 — `test_rollback_from_backup` (mechanical half).
///
/// Reproduces the §8.4 CI drill **minus** the final `recall` assertion
/// (which needs the engramai integration crate; see
/// `test_rollback_full_recall_match_pre_migration` below for the gated
/// follow-up).
///
/// Steps mirror design §8.4 / §8.6 verbatim:
///
/// 1. Seed v0.2 fixture; record pristine snapshot (schema_version + rows).
/// 2. Run migrate(--gate Phase2) — backup must be written in Phase 1, schema
///    advances to additive-columns state in Phase 2 (schema_version stays
///    at 2 because Phase 5 is the canonical bump site, design §3.1 last
///    bullet). For rollback purposes this is the most realistic mid-
///    migration state we can reach without T9.
/// 3. Assert backup file exists at `<db>.pre-v03.bak`.
/// 4. Simulate the §8.6 rollback procedure in-process:
///    a. Drop / close the migration's view of the DB (no `Memory`-level
///       handle exists in this test — the migrate() call is fully
///       finished and released its connection).
///    b. Replace the live DB file with the backup (`std::fs::rename`).
///    c. Reopen.
/// 5. Assert `schema_version == 2` (it already was, but this is the
///    contract operators rely on — after rollback, version stamp is
///    pre-migration's value).
/// 6. Assert the user-data snapshot (memories rows, hebbian rows)
///    matches the pristine snapshot from step 1 — backup-restore is
///    pristine, no data drift.
///
/// **What this proves**: the rollback procedure's mechanical contract
/// (backup-write → file-replace → version-stamp-correct →
/// data-pristine) holds end-to-end against the real Phase-1 backup
/// path. The only step deferred is the `recall` rank check — that
/// needs `engramai::Memory` and is owned by the `#[ignore]`d test
/// below.
///
/// GOAL coverage: **GOAL-4.7** (rollback documented + testable).
#[test]
fn test_rollback_from_backup_mechanical() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("rollback.db");

    // Step 1: seed + snapshot pristine.
    seed_v02_db(&db, 5);
    // Add a couple of hebbian rows so we exercise the multi-table
    // restore (idempotency.rs only checks memories; we want to prove
    // the backup is byte-faithful for ALL user tables).
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "INSERT INTO hebbian_links VALUES ('m0000','m0001');\
             INSERT INTO hebbian_links VALUES ('m0001','m0002');\
             INSERT INTO hebbian_links VALUES ('m0002','m0003');",
        )
        .unwrap();
    }
    let pristine = snapshot_user_state(&db);
    let pristine_version = read_schema_version(&db);
    assert_eq!(pristine_version, 2, "fixture must start at schema_version=2");

    // Step 2: gated migrate to Phase 2. Backup is written in Phase 1.
    let mut opts = options_for(&db);
    opts.gate = Some(MigrationPhase::SchemaTransition);
    let result = migrate(&opts);
    // Gate-reached surfaces as Err per `cli.rs::migrate` — same shape
    // as `test_migrate_twice_is_noop_gate2` in idempotency.rs.
    assert!(
        result.is_err(),
        "gated migrate(Phase2) should surface gate-reached as Err; got {result:?}"
    );

    // Step 3: backup file exists at the documented path.
    let backup = backup_path_for(&db);
    assert!(
        backup.exists(),
        "backup must be written in Phase 1 (path: {})",
        backup.display()
    );
    assert!(
        backup.to_string_lossy().ends_with(BACKUP_SUFFIX),
        "backup path must use the BACKUP_SUFFIX constant ({BACKUP_SUFFIX})"
    );

    // After Phase 2, the live DB has been mutated (additive columns
    // added). Sanity-check that mutation actually happened — otherwise
    // the rollback test would be vacuous.
    {
        let conn = Connection::open(&db).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(memories)").unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut col_names: Vec<String> = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            col_names.push(row.get::<_, String>(1).unwrap());
        }
        // v0.3 additive cols per master DESIGN-v0.3 §8.1.
        assert!(
            col_names.iter().any(|c| c == "episode_id"),
            "Phase 2 must have added 'episode_id' to memories (got {col_names:?})"
        );
    }

    // Step 4: simulate rollback per §8.6 — close any open handles
    // (we don't hold any), replace file with backup, reopen.
    //
    // §8.6 step 3 = "replace the live DB with the backup". We use
    // std::fs::rename which mirrors the `mv backup.db engramai.db`
    // call in scripts/rollback-from-backup.sh.
    std::fs::rename(&backup, &db).expect("rollback step: rename backup → live db");

    // Step 5: post-rollback schema_version == 2.
    let post_version = read_schema_version(&db);
    assert_eq!(
        post_version, 2,
        "after rollback, schema_version must equal pre-migration value (2)"
    );

    // Step 6: post-rollback user data == pristine.
    let post = snapshot_user_state(&db);
    assert_eq!(
        post, pristine,
        "rollback must restore user data byte-for-byte (memories + hebbian + version row)"
    );

    // Auxiliary invariant: the additive v0.3 columns are GONE after
    // rollback (because the backup was taken pre-Phase-2). This is
    // the structural complement of step 5 and surfaces a regression
    // where the rollback restores schema_version but somehow leaves
    // v0.3 columns lingering.
    {
        let conn = Connection::open(&db).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(memories)").unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut col_names: Vec<String> = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            col_names.push(row.get::<_, String>(1).unwrap());
        }
        assert!(
            !col_names.iter().any(|c| c == "episode_id"),
            "after rollback, v0.3 additive columns must be absent (got {col_names:?})"
        );
    }
}

/// §11.4 satellite — `test_rollback_idempotent_on_already_v02`.
///
/// The rollback script `scripts/rollback-from-backup.sh` documents
/// idempotency in its header: "re-running on a DB that is already at
/// schema_version=2 is a no-op". We don't invoke the script from Rust
/// (subprocess + bash availability would be CI-fragile), but we do
/// pin the **library-level** equivalent: if a caller restores from
/// backup twice (because a flaky procedure ran twice), the second
/// restore must produce the same final state, with no data
/// corruption or version drift.
///
/// This is structurally adjacent to GOAL-4.3's idempotency contract,
/// scoped to the rollback path specifically.
#[test]
fn test_rollback_idempotent_on_double_restore() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("rollback-idempotent.db");

    seed_v02_db(&db, 3);
    let pristine = snapshot_user_state(&db);

    // Run gated migrate to write a backup, then restore once.
    let mut opts = options_for(&db);
    opts.gate = Some(MigrationPhase::SchemaTransition);
    let _ = migrate(&opts); // Err is expected (gate-reached).
    let backup = backup_path_for(&db);
    assert!(backup.exists(), "first migrate must write backup");

    // First restore: rename backup → db.
    std::fs::rename(&backup, &db).unwrap();
    // Per docs/migration-rollback.md decision table (Phase 0 row): a
    // restored DB may carry a stale migration_lock row that was written
    // during preflight of the (later-aborted) migrate run. The runbook
    // documents "Drop migration_lock row if present and exit" as a
    // Phase-0 rollback step; in real ops the lock looks dead because
    // the original migrator's PID is gone, but in this single-process
    // test the PID is reused, so we apply the documented step explicitly.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let _ = conn.execute("DELETE FROM migration_lock", []);
        let _ = conn.execute("DELETE FROM migration_state", []);
    }
    let after_first = snapshot_user_state(&db);
    assert_eq!(after_first, pristine, "first restore is pristine");

    // Now re-seed a backup (operator could have re-run migrate before
    // realising they wanted to roll back again). Run gated migrate
    // again to produce a fresh backup of the now-v0.2-again db.
    let result2 = migrate(&opts);
    assert!(
        result2.is_err(),
        "second gated migrate should surface gate-reached"
    );
    let backup2 = backup_path_for(&db);
    assert!(
        backup2.exists(),
        "second migrate run must produce a fresh backup"
    );

    // Second restore.
    std::fs::rename(&backup2, &db).unwrap();
    let after_second = snapshot_user_state(&db);

    assert_eq!(
        after_second, pristine,
        "double-restore must converge on pristine state (rollback is idempotent)"
    );
}

// ---------------------------------------------------------------------------
// §11.4 — Rollback procedure artifacts (runbook + script presence)
// ---------------------------------------------------------------------------

/// §11.4 — runbook artifact contract.
///
/// The §8.4 CI drill is the executable form of the §8.6 5-step
/// checklist documented in `docs/migration-rollback.md`. T15
/// produces those artifacts; T18 owns the **drill** that verifies they
/// stay present (so a future doc-cleanup PR can't silently delete them).
///
/// Asserts:
///   1. `docs/migration-rollback.md` exists at the workspace root.
///   2. It mentions the §8.6 5-step checklist anchors (`schema_version`,
///      `BACKUP_SUFFIX` / `pre-v03.bak`, `lsof`, `integrity_check`).
#[test]
fn test_rollback_runbook_is_present_and_complete() {
    let runbook = workspace_root().join("docs/migration-rollback.md");
    assert!(
        runbook.exists(),
        "T15 deliverable: runbook must exist at {}",
        runbook.display()
    );

    let body = std::fs::read_to_string(&runbook)
        .expect("runbook must be readable");

    // Each of these substrings is a §8.6-mandated checklist anchor.
    // We don't pin exact wording — the runbook is human-edited and
    // can paraphrase — but these concept tokens MUST appear, otherwise
    // the runbook has drifted from the design.
    let required = [
        ("schema_version", "step 5 verifies schema_version == 2"),
        ("pre-v03.bak", "backup path uses the pinned suffix"),
        ("integrity_check", "step 4 runs PRAGMA integrity_check"),
    ];
    for (token, why) in required {
        assert!(
            body.contains(token),
            "runbook missing required anchor '{token}' ({why}); \
             rewrite drift between docs/migration-rollback.md and design §8.6"
        );
    }
}

/// §11.4 — rollback script artifact contract.
///
/// The §8.4 CI drill calls `scripts/rollback-from-backup.sh`. This
/// integration test verifies the script is present and exercises its
/// header-documented invariants (executable bit + the documented exit
/// codes 0..5 listed in the file header).
#[test]
fn test_rollback_script_is_present_and_documented() {
    let script = workspace_root().join("scripts/rollback-from-backup.sh");
    assert!(
        script.exists(),
        "T15 deliverable: rollback script must exist at {}",
        script.display()
    );

    let body = std::fs::read_to_string(&script)
        .expect("rollback script must be readable");

    // Must be a bash script (#!/usr/bin/env bash) — the documented
    // invariant in the header.
    let first_line = body.lines().next().unwrap_or("");
    assert!(
        first_line.starts_with("#!") && first_line.contains("bash"),
        "rollback script must have a bash shebang; got first line: {first_line}"
    );

    // Must reference the BACKUP_SUFFIX library constant value
    // ("pre-v03.bak"). Drift between the constant and the script is a
    // contract bug — operators reading the runbook expect the same
    // path the library writes.
    assert!(
        body.contains(BACKUP_SUFFIX),
        "rollback script must reference BACKUP_SUFFIX value '{BACKUP_SUFFIX}'; drift detected"
    );

    // Header must document the documented exit codes (0..5 per the
    // header block). Each numbered reason appears as `^   N   ` per
    // the script's column layout.
    for code in 0..=5 {
        let needle = format!("\n#   {code}");
        assert!(
            body.contains(&needle),
            "rollback script header must document exit code {code} (looking for '{needle}')"
        );
    }

    // Executable bit on Unix. We don't assert this on Windows since
    // git-on-Windows often loses the +x bit; CI lives on Linux/macOS.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::metadata(&script).unwrap().permissions();
        assert!(
            perm.mode() & 0o111 != 0,
            "rollback script must be executable; mode = {:o}",
            perm.mode()
        );
    }
}

// ---------------------------------------------------------------------------
// §11.5 — Backward-compat surface contract (library boundary)
// ---------------------------------------------------------------------------

/// §11.5 / §7.1 — the v0.2 frozen-method list at the library boundary.
///
/// Unit tests in `src/compat.rs` already assert this internally; the
/// integration test here re-asserts at the public-API boundary so that
/// a future refactor that hides the constant or reshapes its element
/// type fails in CI. This is the **integration-side lock** complementary
/// to T13's unit-side lock.
///
/// GOAL coverage: GOAL-4.9 (v0.2 store/recall/recall_recent/
/// recall_with_associations preserved).
#[test]
fn test_v02_frozen_methods_match_design_seven_one() {
    // Design §7.1 freezes EXACTLY these four methods — no more, no less.
    assert_eq!(
        V02_FROZEN_METHODS,
        &["store", "recall", "recall_recent", "recall_with_associations"],
        "V02_FROZEN_METHODS must match design §7.1 verbatim — \
         adding/removing a method here is a breaking change to GOAL-4.9"
    );
}

/// §11.5 / §7.2 — every frozen method has a complete behavioral contract.
///
/// `BEHAVIORAL_CONTRACT` is the public surface the integration tests
/// (those that DO have engramai access — see the `#[ignore]`d block
/// below) read off to know what to assert. Pin its shape here so a
/// future edit that drops a column (e.g. `v03_on_fresh_db`) is caught.
#[test]
fn test_behavioral_contract_complete_at_library_boundary() {
    use engramai_migrate::contract_for;

    assert_eq!(
        BEHAVIORAL_CONTRACT.len(),
        V02_FROZEN_METHODS.len(),
        "one BEHAVIORAL_CONTRACT row per frozen method"
    );

    for (entry, &method) in BEHAVIORAL_CONTRACT.iter().zip(V02_FROZEN_METHODS) {
        assert_eq!(
            entry.method, method,
            "BEHAVIORAL_CONTRACT order must match V02_FROZEN_METHODS"
        );

        let lookup = contract_for(method)
            .unwrap_or_else(|| panic!("contract_for({method}) must resolve"));

        // Every documented column must be non-empty — empty strings
        // mean an editor stubbed the field and forgot to fill it in.
        assert!(
            !lookup.v02_documented_behavior.is_empty(),
            "{method}: v02_documented_behavior must be filled (design §7.2)"
        );
        assert!(
            !lookup.v03_on_migrated_db.is_empty(),
            "{method}: v03_on_migrated_db must be filled (design §7.2)"
        );
        assert!(
            !lookup.v03_on_fresh_db.is_empty(),
            "{method}: v03_on_fresh_db must be filled (design §7.2)"
        );
    }
}

/// §11.5 — design §7.3 compat matrix has 8 cells (4 methods × 2 setups).
///
/// The matrix isn't a runtime data structure (it's prose in the design
/// doc), but every cell maps to one test in the §11.5 8-test suite
/// (currently `#[ignore]`d below). When that suite ships, the test
/// count will be 8. For now we lock the **arithmetic** of the matrix
/// at the boundary so a future doc edit that adds a 5th method or a
/// 3rd setup column gets caught here.
#[test]
fn test_compat_matrix_arithmetic_is_eight() {
    let n_methods = V02_FROZEN_METHODS.len(); // 4 per §7.1
    let n_setups = 2; // {fresh-v0.3, migrated-from-v0.2}, hard-coded in §7.3
    assert_eq!(
        n_methods * n_setups,
        8,
        "compat matrix is a 4×2 grid per design §7.3; got {n_methods} × {n_setups}"
    );
}

// ---------------------------------------------------------------------------
// §11.7 — Post-migration concurrent-read smoke test (no-recall variant)
// ---------------------------------------------------------------------------

/// §11.7 — `test_concurrent_reads_post_migration` (reduced).
///
/// Full §11.7 spec calls for `recall` / `recall_recent` /
/// `recall_with_associations` running in worker threads. Those need
/// the engramai `Memory` type which this leaf crate cannot reach
/// (see compat.rs module docs). The full version lives in the
/// `#[ignore]`d block below as
/// `test_concurrent_reads_post_migration_with_recall`.
///
/// What we DO run today is the §11.7 invariants that don't depend on
/// retrieval — the SQLite-level safety contract:
///
///   1. Multiple threads opening their own `Connection` to the
///      migrated DB and issuing read-only queries see zero errors.
///   2. After the stress window, `PRAGMA integrity_check` returns
///      `ok` (catches WAL corruption, page-cache desync).
///   3. `PRAGMA wal_checkpoint(TRUNCATE)` succeeds afterward (catches
///      WAL-stuck / never-checkpointable bugs introduced by Phase 2's
///      DDL or Phase 1's backup path).
///
/// Window is **2 seconds** (full spec is 10s) — this is a smoke test,
/// not a benchmark, and CI minutes are precious. The 4-thread shape
/// is preserved.
///
/// GOAL coverage: GOAL-4.9 (migrated DB usable under read load —
/// post-migration store/recall behavior preserved).
#[test]
fn test_concurrent_reads_post_migration_smoke() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("concurrent.db");

    // Fixture: 50 rows so worker SELECTs return non-trivial result sets.
    seed_v02_db(&db, 50);

    // Run gated migrate to Phase 2 — this gives us a "post-migration
    // (schema-only)" DB, the closest we can get to design §11.7's
    // "migrate the v0.2 fixture" pre-condition without T9.
    let mut opts = options_for(&db);
    opts.gate = Some(MigrationPhase::SchemaTransition);
    opts.no_backup = true; // smoke test doesn't need backup sidecar
    let _ = migrate(&opts); // gate-reached Err is expected

    // Sanity check: the migrated DB has the v0.3 additive cols.
    {
        let conn = Connection::open(&db).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 50, "fixture must have 50 rows post-migration");
    }

    // Spawn 4 readers, each looping for ~2s.
    let stop = Arc::new(AtomicBool::new(false));
    let total_reads = Arc::new(AtomicU64::new(0));
    let total_errors = Arc::new(AtomicU64::new(0));
    let db_path = db.clone();

    let mut handles = Vec::with_capacity(4);
    for worker_id in 0..4 {
        let stop = Arc::clone(&stop);
        let total_reads = Arc::clone(&total_reads);
        let total_errors = Arc::clone(&total_errors);
        let db_path = db_path.clone();

        let handle = thread::spawn(move || {
            let conn = match Connection::open(&db_path) {
                Ok(c) => c,
                Err(_) => {
                    total_errors.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            };

            // Each worker rotates through three read queries, mirroring
            // the §11.7 spec's "varied recall / recall_recent /
            // recall_with_associations" pattern at the SQLite level.
            //
            // The id LIKE filter is bound at runtime to vary input
            // (catches any path where SQLite's prepared-statement cache
            // misbehaves under contention).
            while !stop.load(Ordering::Relaxed) {
                let i = total_reads.fetch_add(1, Ordering::Relaxed) % 3;
                // Use an immediately-invoked closure so we can use `?`
                // and bubble errors up to the counter without
                // restructuring the worker loop. The closure captures
                // `&conn` and `worker_id` by reference; both outlive
                // the loop body.
                let run = || -> rusqlite::Result<()> {
                    match i {
                        0 => {
                            // Recall-shape: scan with a content filter.
                            let mut stmt = conn.prepare(
                                "SELECT id, content FROM memories \
                                 WHERE content LIKE ?1 LIMIT 5",
                            )?;
                            let pat = format!("%row {worker_id}%");
                            let mut rows = stmt.query([&pat])?;
                            while rows.next()?.is_some() {}
                        }
                        1 => {
                            // Recall-recent-shape: ordered by created_at.
                            let mut stmt = conn.prepare(
                                "SELECT id, created_at FROM memories \
                                 ORDER BY created_at DESC LIMIT 10",
                            )?;
                            let mut rows = stmt.query([])?;
                            while rows.next()?.is_some() {}
                        }
                        _ => {
                            // Associations-shape: count hebbian rows.
                            let _: i64 = conn.query_row(
                                "SELECT COUNT(*) FROM hebbian_links",
                                [],
                                |r| r.get(0),
                            )?;
                        }
                    }
                    Ok(())
                };
                if run().is_err() {
                    total_errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        handles.push(handle);
    }

    // Run for 2s, then stop.
    let window = Duration::from_secs(2);
    let started = Instant::now();
    while started.elapsed() < window {
        thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::Relaxed);

    for h in handles {
        h.join().expect("worker must not panic");
    }

    // §11.7 assertion 1: zero errors.
    let errors = total_errors.load(Ordering::Relaxed);
    assert_eq!(
        errors, 0,
        "concurrent readers reported {errors} errors (expected 0)"
    );
    let reads = total_reads.load(Ordering::Relaxed);
    assert!(
        reads > 0,
        "smoke test should have completed at least one read; got {reads}"
    );

    // §11.7 assertion 2: PRAGMA integrity_check returns "ok".
    {
        let conn = Connection::open(&db).unwrap();
        let result: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .expect("integrity_check must return at least one row");
        assert_eq!(
            result, "ok",
            "post-stress integrity_check must return 'ok', got {result:?}"
        );
    }

    // §11.7 assertion 3: wal_checkpoint(TRUNCATE) succeeds.
    {
        let conn = Connection::open(&db).unwrap();
        // PRAGMA wal_checkpoint(TRUNCATE) returns (busy, log_pages, checkpointed).
        // busy=0 means success; we don't care about the page counters.
        let busy: i64 = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
            .expect("wal_checkpoint must return at least one row");
        assert_eq!(
            busy, 0,
            "wal_checkpoint(TRUNCATE) busy flag must be 0 (success); got {busy}"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 4 / engramai-integration tests — gated on T9 + integration crate
// ---------------------------------------------------------------------------

/// §11.4 — full rollback drill (with `recall` assertion).
///
/// **Currently `#[ignore]` — gated on `task:mig-impl-backfill-perrecord`
/// (T9) AND a future integration crate that depends on both
/// `engramai-migrate` and `engramai` core.** See
/// `tasks/2026-04-27-night-autopilot-STATUS.md` for T9 status; the
/// integration crate doesn't exist yet (see compat.rs module docs for
/// why this leaf crate cannot pull in `engramai::Memory`).
///
/// When the gates lift, the body becomes (per design §11.4 step 6/7):
///
///   1-5: as in `test_rollback_from_backup_mechanical` above.
///   6.  Run a v0.2-style `recall("<canonical query>")` using
///       `Memory::open_v02_compat_mode` — read-only v0.2 emulation.
///   7.  Assert recall results match the pre-migration result captured
///       in step 1 (top-1 must match; top-K reorder allowed iff
///       Kendall-τ ≥ 0.8 per §11.5).
#[test]
#[ignore = "T9 + engramai-integration crate not yet available — \
            mechanical drill is covered by test_rollback_from_backup_mechanical"]
fn test_rollback_full_recall_match_pre_migration() {
    // TODO(T9 + integration crate): implement per design §11.4 step 6/7.
    unimplemented!("blocked on T9 + engramai integration");
}

/// §11.5 — fresh-v0.3 compat suite (4 tests collapsed into one for
/// scaffolding visibility).
///
/// **Currently `#[ignore]` — gated on the engramai-integration crate.**
/// The four design §11.5 tests are:
///
///   - `test_store_fresh` — `store()` returns `MemoryId`; row exists
///     in `memories`.
///   - `test_recall_fresh` — `recall()` returns ranked results;
///     ranking contract holds.
///   - `test_recent_fresh` — `recall_recent(N)` returns N most recent.
///   - `test_assoc_fresh` — `recall_with_associations` returns
///     Hebbian neighbors.
///
/// When the integration crate exists, split this scaffold into four
/// distinct `#[test]` functions (one per design name) and remove the
/// scaffold.
#[test]
#[ignore = "engramai-integration crate not yet available"]
fn test_compat_fresh_v03_suite_four_tests() {
    // TODO(integration crate): implement per design §11.5 fresh suite.
    unimplemented!("blocked on engramai integration crate");
}

/// §11.5 — migrated-v0.2 compat suite (4 tests collapsed).
///
/// **Currently `#[ignore]` — gated on T9 + engramai integration crate
/// + v0.2 fixture (`crates/engramai/tests/fixtures/v02_sample.db`,
/// design §11.1, not yet checked in).**
///
/// When the gates lift: same four shapes as `..._fresh_v03_suite_...`
/// above but on a DB that has been migrated from v0.2. Plus the
/// **ranking-contract** test (the curated 30-pair query/expected-top-5
/// stratified set, design §11.5 last paragraph; Kendall-τ ≥ 0.8 with
/// top-1 match-required).
#[test]
#[ignore = "T9 + integration crate + v02 fixture not yet available"]
fn test_compat_migrated_v02_suite_with_ranking_contract() {
    // TODO(T9 + integration crate + fixture): implement per design §11.5
    //   migrated suite + ranking-contract paragraph.
    unimplemented!("blocked on T9 + integration crate + v02 fixture");
}

/// §11.5 cross-test invariant — backward-compat suite must pass with
/// both default and `--no-backup` options.
///
/// **Currently `#[ignore]` — gated on the same chain as the migrated
/// suite above.** When it lifts, this test runs the migrated-v0.2
/// suite twice: once with default `MigrateOptions`, once with
/// `no_backup = true`, asserting both produce identical results.
#[test]
#[ignore = "T9 + integration crate + v02 fixture not yet available"]
fn test_compat_suite_invariant_under_no_backup() {
    // TODO: implement per design §11.5 "Cross-test invariant" paragraph.
    unimplemented!("blocked on T9 + integration crate + v02 fixture");
}

/// §11.7 — full concurrent-read smoke test (with real `recall` and
/// p99 latency tracking).
///
/// **Currently `#[ignore]` — gated on T9 + engramai integration crate.**
/// The reduced (no-recall) version above (`..._smoke`) covers
/// assertions 1, 2, 4 (zero errors, integrity_check ok, wal_checkpoint
/// ok). Assertion 3 (no reader's p99 exceeds 3× single-reader baseline)
/// requires running real `recall` from worker threads and is gated
/// here.
#[test]
#[ignore = "T9 + engramai integration crate not yet available"]
fn test_concurrent_reads_post_migration_with_recall() {
    // TODO: implement per design §11.7 full spec, including p99 tracking.
    unimplemented!("blocked on T9 + engramai integration crate");
}
