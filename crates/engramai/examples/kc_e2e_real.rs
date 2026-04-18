//! KC End-to-End test with real engram DB.
//!
//! Run: cargo run --example kc_e2e_real --features kc -- /path/to/engram-memory.db

use std::path::Path;
use chrono::{DateTime, Utc, NaiveDateTime};
use rusqlite::Connection;

use engramai::compiler::{
    SqliteKnowledgeStore, KnowledgeStore,
    TopicId, TopicStatus, KcConfig,
};
use engramai::compiler::api::MaintenanceApi;
use engramai::compiler::compilation::MemorySnapshot;
use engramai::compiler::llm::NoopProvider;

fn main() {
    let db_path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: cargo run --example kc_e2e_real --features kc -- <engram-db-path>");
        std::process::exit(1);
    });

    println!("═══════════════════════════════════════════════════════");
    println!("  KC End-to-End Test — Real Engram DB");
    println!("═══════════════════════════════════════════════════════");
    println!("DB: {}", db_path);
    println!();

    // ── Step 1: Read memories from real DB ──────────────────────────────
    let engram_conn = Connection::open(&db_path).expect("Failed to open engram DB");

    let total: i64 = engram_conn
        .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
        .expect("count");
    println!("📊 Total memories in DB: {}", total);

    // Check if memory_embeddings table exists
    let has_embeddings: bool = engram_conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='memory_embeddings'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    let embed_count: i64 = if has_embeddings {
        engram_conn
            .query_row("SELECT COUNT(*) FROM memory_embeddings", [], |r| r.get(0))
            .unwrap_or(0)
    } else {
        0
    };
    println!("🧮 Memories with embeddings: {}", embed_count);
    println!();

    // Load memories with embeddings (sample up to 500 for performance)
    let mut stmt = engram_conn
        .prepare(
            "SELECT m.id, m.content, m.memory_type, m.importance, m.created_at
             FROM memories m
             INNER JOIN memory_embeddings e ON m.id = e.memory_id
             ORDER BY m.importance DESC, m.created_at DESC
             LIMIT 100",
        )
        .expect("prepare");

    let memories: Vec<MemorySnapshot> = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let memory_type: String = row.get(2)?;
            let importance: f64 = row.get(3)?;
            let created_at_epoch: f64 = row.get(4)?;

            Ok((id, content, memory_type, importance, created_at_epoch))
        })
        .expect("query")
        .filter_map(|r| r.ok())
        .map(|(id, content, memory_type, importance, created_at_epoch)| {
            let created_at = epoch_to_datetime(created_at_epoch);
            // Load embedding for this memory
            let embedding: Option<Vec<f32>> = if has_embeddings {
                engram_conn
                    .query_row(
                        "SELECT embedding FROM memory_embeddings WHERE memory_id = ?1",
                        [&id],
                        |row| {
                            let blob: Vec<u8> = row.get(0)?;
                            Ok(bytes_to_f32_vec(&blob))
                        },
                    )
                    .ok()
            } else {
                None
            };

            MemorySnapshot {
                id,
                content,
                memory_type,
                importance,
                created_at,
                updated_at: created_at,  // engram schema has no updated_at
                tags: vec![],
                embedding,
            }
        })
        .collect();

    println!("📥 Loaded {} memories (with embeddings, top by importance)", memories.len());
    if let Some(m) = memories.first() {
        println!("   Top memory: [{}] imp={:.2} — {}",
            m.memory_type,
            m.importance,
            truncate(&m.content, 80));
    }
    println!();

    // ── Step 2: Set up KC store (separate DB for topic pages) ───────────
    let kc_db_path = format!("{}.kc-test.db", db_path);
    let kc_store = SqliteKnowledgeStore::open(Path::new(&kc_db_path))
        .expect("Failed to create KC store");
    kc_store.init_schema().expect("Failed to init KC schema");

    let config = KcConfig::default();
    let api = MaintenanceApi::new(kc_store, config);

    println!("🗄️  KC store: {}", kc_db_path);
    println!();

    // ── Step 3: Dry Run — see what would happen ─────────────────────────
    println!("═══════════════════════════════════════════════════════");
    println!("  Phase 1: DRY RUN");
    println!("═══════════════════════════════════════════════════════");

    match api.dry_run(&memories) {
        Ok(report) => {
            println!("📋 Dry run results:");
            println!("   Total entries: {}", report.entries.len());
            println!("   Topics affected: {}", report.total_topics_affected);
            println!("   Estimated LLM calls: {}", report.estimated_llm_calls);
            println!();
            for (i, entry) in report.entries.iter().enumerate().take(20) {
                println!("   [{:2}] {:?} — {} memories — {}",
                    i + 1,
                    entry.action,
                    entry.affected_memories,
                    entry.reason);
            }
            if report.entries.len() > 20 {
                println!("   ... and {} more", report.entries.len() - 20);
            }
        }
        Err(e) => {
            println!("❌ Dry run failed: {:?}", e);
        }
    }
    println!();

    // ── Step 4: Compile (without LLM) ───────────────────────────────────
    println!("═══════════════════════════════════════════════════════");
    println!("  Phase 2: COMPILE (no LLM)");
    println!("═══════════════════════════════════════════════════════");

    // We pass None for LLM — uses compile_without_llm path
    let noop = NoopProvider;
    match api.compile_all(Some(&noop), &memories) {
        Ok(pages) => {
            println!("✅ Compiled {} topic pages", pages.len());
            println!();
            for (i, page) in pages.iter().enumerate().take(20) {
                println!("   [{:2}] {} ({})", i + 1, page.title, page.id);
                println!("        Summary: {}", truncate(&page.summary, 100));
                println!("        Sources: {} memories", page.metadata.source_memory_ids.len());
                println!("        Content: {} chars", page.content.len());
                println!();
            }
            if pages.len() > 20 {
                println!("   ... and {} more topic pages", pages.len() - 20);
            }
        }
        Err(e) => {
            println!("❌ Compilation failed: {:?}", e);
        }
    }

    // ── Step 5: List compiled topics ────────────────────────────────────
    println!("═══════════════════════════════════════════════════════");
    println!("  Phase 3: LIST TOPICS");
    println!("═══════════════════════════════════════════════════════");

    match api.list() {
        Ok(pages) => {
            println!("📚 {} topic pages in store", pages.len());
            for page in &pages {
                println!("   • {} [{}] — {} chars",
                    page.title,
                    match page.status {
                        TopicStatus::Active => "Active",
                        TopicStatus::Stale => "Stale",
                        TopicStatus::Archived => "Archived",
                        TopicStatus::FailedPermanent => "FailedPerm",
                    },
                    page.content.len());
            }
        }
        Err(e) => {
            println!("❌ List failed: {:?}", e);
        }
    }
    println!();

    // ── Step 6: Query ───────────────────────────────────────────────────
    println!("═══════════════════════════════════════════════════════");
    println!("  Phase 4: QUERY");
    println!("═══════════════════════════════════════════════════════");

    let queries = ["RustClaw", "engram memory", "financial freedom", "KC knowledge compiler"];
    for q in &queries {
        let opts = engramai::compiler::api::QueryOpts::default();
        match api.query(q, &opts) {
            Ok(results) => {
                println!("🔍 \"{}\" → {} results", q, results.len());
                for r in results.iter().take(3) {
                    println!("     {:.2} — {} — {}", r.relevance, r.title, truncate(&r.summary, 80));
                }
            }
            Err(e) => {
                println!("🔍 \"{}\" → error: {:?}", q, e);
            }
        }
        println!();
    }

    // ── Step 7: Health Report ───────────────────────────────────────────
    println!("═══════════════════════════════════════════════════════");
    println!("  Phase 5: HEALTH REPORT");
    println!("═══════════════════════════════════════════════════════");

    match api.health_report() {
        Ok(report) => {
            println!("🏥 Health Report:");
            println!("   Total topics: {}", report.total_topics);
            println!("   Stale topics: {}", report.stale_topics.len());
            println!("   Conflicts: {}", report.conflicts.len());
            println!("   Broken links: {}", report.broken_links.len());
            println!("   Recommendations: {}", report.recommendations.len());
            for rec in &report.recommendations {
                println!("     • {:?}", rec);
            }
        }
        Err(e) => {
            println!("❌ Health report failed: {:?}", e);
        }
    }
    println!();

    // ── Step 8: Conflict Detection ──────────────────────────────────────
    println!("═══════════════════════════════════════════════════════");
    println!("  Phase 6: CONFLICT DETECTION");
    println!("═══════════════════════════════════════════════════════");

    match api.detect_conflicts() {
        Ok(conflicts) => {
            if conflicts.is_empty() {
                println!("✅ No conflicts detected");
            } else {
                println!("⚠️  {} conflicts found:", conflicts.len());
                for c in conflicts.iter().take(10) {
                    println!("   • {:?}", c);
                }
            }
        }
        Err(e) => {
            println!("❌ Conflict detection failed: {:?}", e);
        }
    }
    println!();

    // Cleanup test DB
    println!("═══════════════════════════════════════════════════════");
    println!("  CLEANUP");
    println!("═══════════════════════════════════════════════════════");
    drop(api);
    if let Err(e) = std::fs::remove_file(&kc_db_path) {
        println!("⚠️  Could not remove test DB: {}", e);
    } else {
        println!("🧹 Removed test DB: {}", kc_db_path);
    }

    println!();
    println!("✅ KC E2E test complete!");
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn epoch_to_datetime(epoch: f64) -> DateTime<Utc> {
    DateTime::from_timestamp(epoch as i64, ((epoch.fract()) * 1_000_000_000.0) as u32)
        .unwrap_or_else(|| Utc::now())
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    // Try RFC3339 first, then common SQLite formats
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return dt.with_timezone(&Utc);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return dt.and_utc();
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return dt.and_utc();
    }
    Utc::now() // fallback
}

fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..s.char_indices().take(max).last().map(|(i, _)| i).unwrap_or(0)])
    }
}

// Uses NoopProvider from engramai::compiler::llm
