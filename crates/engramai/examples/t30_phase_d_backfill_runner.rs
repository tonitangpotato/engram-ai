//! T30 — Phase D parity prep: run all Phase C backfill drivers against
//! a production-DB snapshot.
//!
//! Backfill is a prerequisite for any `unified_substrate=true` read to
//! return non-empty results on legacy data. The 7 Phase C drivers
//! (T19–T25) plus T26's soft-delete projection have public APIs but
//! no batched CLI entry point — this example is the operational
//! wrapper that drives them in dependency order and prints an audit
//! summary.
//!
//! # Driver order (dependency-correct, design §5.3)
//!
//! 1. `backfill_memories_to_nodes`         (T19) — base nodes
//! 2. `backfill_embeddings_to_node_embeddings` (T20) — needs (1)
//! 3. `backfill_entities_to_nodes`         (T21) — entity nodes
//! 4. `backfill_entity_relations_to_edges` (T22) — needs (3)
//! 5. `backfill_memory_entities_to_edges`  (T23) — needs (1)+(3)
//! 6. `backfill_hebbian_links_to_edges`    (T24) — needs (1)
//! 7. `backfill_synthesis_provenance_to_edges` (T25) — needs (1)
//! 8. `backfill_soft_delete_into_nodes`    (T26 soft-delete) — needs (1)
//!
//! # Usage
//!
//! ```bash
//! cargo run --release --example t30_phase_d_backfill_runner -- \
//!   --source /tmp/t30-probe.db
//! ```
//!
//! `--source` MUST be a snapshot (not the live engram-memory.db).
//! Running against a live DB is rejected because the backfills hold
//! write locks for minutes on 12k+ memories.
//!
//! # Exit codes
//! - 0: all 8 drivers completed with `rows_failed == 0`
//! - 1: at least one driver returned an error or non-zero failures
//! - 2: setup error (missing arg, source not found, etc.)

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use engramai::storage::Storage;
use engramai::substrate::backfill::{
    backfill_embeddings_to_node_embeddings, backfill_entities_to_nodes,
    backfill_entity_relations_to_edges, backfill_hebbian_links_to_edges,
    backfill_memories_to_nodes, backfill_memory_entities_to_edges, backfill_soft_delete_into_nodes,
    backfill_synthesis_provenance_to_edges, BackfillRun,
};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let source = match parse_source(&args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("error: {msg}");
            eprintln!("usage: t30_phase_d_backfill_runner --source <path-to-snapshot.db>");
            return ExitCode::from(2);
        }
    };

    if source
        .to_string_lossy()
        .contains("rustclaw/engram-memory.db")
        && !source.to_string_lossy().contains(".bak")
        && !source.to_string_lossy().starts_with("/tmp/")
    {
        eprintln!(
            "refusing to run against live production DB: {}",
            source.display()
        );
        eprintln!("snapshot it first: cp engram-memory.db /tmp/t30-probe.db");
        return ExitCode::from(2);
    }

    if !source.exists() {
        eprintln!("source DB not found: {}", source.display());
        return ExitCode::from(2);
    }

    println!("T30 Phase D backfill runner");
    println!("source: {}", source.display());
    println!();

    // Open with unified_substrate=false so we read from legacy and
    // write the unified projections via the backfill drivers
    // themselves. The flag only affects READ-side adapter routing,
    // not the schema — both legacy and unified tables exist either
    // way after migrations run.
    let path_str = source.to_str().expect("snapshot path must be UTF-8");
    let mut storage = match Storage::with_unified_substrate(path_str, false) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Storage::with_unified_substrate failed: {e}");
            return ExitCode::from(1);
        }
    };

    type DriverFn = fn(&mut Storage, Option<&str>) -> Result<BackfillRun, rusqlite::Error>;
    let drivers: &[(&str, DriverFn)] = &[
        ("T19 memories→nodes", backfill_memories_to_nodes),
        (
            "T20 embeddings→node_embeddings",
            backfill_embeddings_to_node_embeddings,
        ),
        ("T21 entities→nodes", backfill_entities_to_nodes),
        (
            "T22 entity_relations→edges",
            backfill_entity_relations_to_edges,
        ),
        (
            "T23 memory_entities→edges",
            backfill_memory_entities_to_edges,
        ),
        ("T24 hebbian_links→edges", backfill_hebbian_links_to_edges),
        (
            "T25 synthesis_provenance→edges",
            backfill_synthesis_provenance_to_edges,
        ),
        (
            "T26 soft_delete projection",
            backfill_soft_delete_into_nodes,
        ),
    ];

    let mut any_failed = false;
    let total_started = Instant::now();
    let mut summary_lines = Vec::new();

    for (label, driver) in drivers {
        println!("== {label} ==");
        let t0 = Instant::now();
        match driver(&mut storage, None) {
            Ok(run) => {
                let elapsed = t0.elapsed().as_secs_f64();
                println!(
                    "  run_id={} read={} inserted={} skipped={} failed={} ({:.2}s)",
                    &run.run_id[..8],
                    run.rows_read,
                    run.rows_inserted,
                    run.rows_skipped_existing,
                    run.rows_failed,
                    elapsed
                );
                if run.rows_failed > 0 {
                    any_failed = true;
                    println!("  WARNING: rows_failed > 0");
                }
                summary_lines.push(format!(
                    "{:35} read={:>6} inserted={:>6} skipped={:>6} failed={:>4} {:>6.2}s",
                    label,
                    run.rows_read,
                    run.rows_inserted,
                    run.rows_skipped_existing,
                    run.rows_failed,
                    elapsed
                ));
            }
            Err(e) => {
                println!("  ERROR: {e}");
                any_failed = true;
                summary_lines.push(format!("{:35} ERROR: {e}", label));
            }
        }
        println!();
    }

    let total_elapsed = total_started.elapsed().as_secs_f64();
    println!("=== Summary ===");
    for line in &summary_lines {
        println!("  {line}");
    }
    println!();
    println!("total wall-clock: {:.2}s", total_elapsed);

    if any_failed {
        eprintln!("FAIL: one or more drivers errored or reported rows_failed > 0");
        ExitCode::from(1)
    } else {
        println!("OK: all drivers completed cleanly");
        ExitCode::SUCCESS
    }
}

fn parse_source(args: &[String]) -> Result<PathBuf, String> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--source" {
            let val = iter.next().ok_or("--source requires a value")?;
            return Ok(Path::new(val).to_path_buf());
        }
    }
    Err("--source <path> is required".to_string())
}
