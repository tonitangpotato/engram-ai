//! ISS-132 / ISS-044 diagnostic: reproduce the iss044_backfill failure and
//! dump `graph_extraction_failures` so we can see *which* of m1/m2/m3 failed
//! and *why*.

use std::path::Path;

use rusqlite::Connection;

use engramai_migrate::{migrate, MigrateOptions};

fn seed_v02(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE memories (\
             id TEXT PRIMARY KEY,\
             content TEXT,\
             metadata TEXT,\
             created_at TEXT\
         );\
         INSERT INTO memories VALUES ('m1', 'Filed ISS-100 against gid-rs to track src/main.rs refactor.', NULL, '2026-01-01T00:00:00Z');\
         INSERT INTO memories VALUES ('m2', 'See https://example.com/issue/200 for ISS-200 details from @alice_dev.', NULL, '2026-01-02T00:00:00Z');\
         INSERT INTO memories VALUES ('m3', 'Updated GOAL-3.1 in design.md and tracked GUARD-7 against engramai-rs.', NULL, '2026-01-03T00:00:00Z');\
         CREATE TABLE hebbian_links (a TEXT, b TEXT);\
         CREATE TABLE schema_version (version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
         INSERT INTO schema_version VALUES (2, '2026-01-01T00:00:00Z');",
    )
    .unwrap();
}

fn main() {
    // Use /tmp directly instead of tempfile crate (it's dev-dep only).
    let dir = std::path::PathBuf::from(format!("/tmp/iss132-diag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("populated.db");
    let graph_db = dir.join("populated.graph.db");
    seed_v02(&db);

    let mut opts = MigrateOptions::new(&db);
    opts.tool_version = "0.1.0-iss132-diag".to_string();
    opts.accept_forward_only = true;
    opts.no_backup = true;
    opts.accept_no_grace = true;
    opts.graph_db_path = Some(graph_db.clone());

    let report = migrate(&opts).expect("migrate must succeed");
    println!("\n=== BackfillReport ===");
    println!("{:#?}", report.backfill);

    // Post-ISS-058: graph_extraction_failures lives in the GRAPH DB only.
    println!("\n=== graph_extraction_failures (graph DB) ===");
    let gconn = Connection::open(&graph_db).unwrap();
    let mut stmt = gconn
        .prepare(
            "SELECT hex(episode_id), stage, error_category, \
             substr(error_detail, 1, 800), retry_count, resolved_at, namespace \
             FROM graph_extraction_failures",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<f64>>(5)?,
                r.get::<_, String>(6)?,
            ))
        })
        .unwrap();
    let mut n = 0;
    for row in rows {
        n += 1;
        let (ep, stage, cat, detail, retry, resolved, ns) = row.unwrap();
        println!(
            "[{n}] episode_id={}\n    stage={} category={} retry={} resolved_at={:?} ns={}\n    detail={}",
            ep,
            stage,
            cat,
            retry,
            resolved,
            ns,
            detail.unwrap_or_else(|| "<null>".into())
        );
    }
    if n == 0 {
        println!("  (no rows — failure not surfaced via graph_extraction_failures)");
    }

    // Also dump backfill_runs.notes (ISS-128) to catch any failed_memory_ids
    // persisted there.
    println!("\n=== backfill_runs (graph DB) ===");
    let table_exists: bool = gconn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='backfill_runs'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !table_exists {
        println!("  (table backfill_runs not present)");
        return;
    }
    // backfill_runs column set varies by migration era — discover columns first.
    let mut cols_stmt = gconn.prepare("PRAGMA table_info(backfill_runs)").unwrap();
    let cols: Vec<String> = cols_stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    println!("  columns: {:?}", cols);
    let sql = format!("SELECT * FROM backfill_runs");
    let mut stmt = gconn.prepare(&sql).unwrap();
    let col_count = stmt.column_count();
    let mut rows = stmt.query([]).unwrap();
    let mut row_n = 0;
    while let Some(row) = rows.next().unwrap() {
        row_n += 1;
        print!("  row[{row_n}]:");
        for i in 0..col_count {
            let name = &cols[i];
            // Try string then i64 then f64 then null
            let val: String = row
                .get::<_, Option<String>>(i)
                .ok()
                .flatten()
                .or_else(|| {
                    row.get::<_, Option<i64>>(i)
                        .ok()
                        .flatten()
                        .map(|x| x.to_string())
                })
                .or_else(|| {
                    row.get::<_, Option<f64>>(i)
                        .ok()
                        .flatten()
                        .map(|x| x.to_string())
                })
                .unwrap_or_else(|| "<null>".into());
            let val = if val.len() > 200 {
                format!("{}…(truncated)", &val[..200])
            } else {
                val
            };
            print!(" {}={}", name, val);
        }
        println!();
    }
    if row_n == 0 {
        println!("  (empty)");
    }
}
