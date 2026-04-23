//! Full benchmark with per-phase timing.
use std::time::Instant;

fn main() {
    let db_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/Users/potato/rustclaw/engram-memory.db".to_string());

    println!("=== Synthesis Cluster Discovery Benchmark ===");
    println!("DB: {}", db_path);

    let storage = engramai::storage::Storage::new(&db_path).expect("open");

    let all = storage.all().expect("all");
    println!("Total memories: {}", all.len());

    let config = engramai::synthesis::types::ClusterDiscoveryConfig::default();
    let candidates: Vec<_> = all
        .iter()
        .filter(|m| {
            !m.access_times.is_empty()
                && m.importance >= config.min_importance
                && !m.metadata
                    .as_ref()
                    .and_then(|md: &serde_json::Value| md.get("is_synthesis"))
                    .and_then(|v: &serde_json::Value| v.as_bool())
                    .unwrap_or(false)
        })
        .collect();
    println!("Candidates: {}", candidates.len());

    // Run discover_clusters with timing
    println!("\n--- discover_clusters (full pipeline) ---");
    let total_start = Instant::now();
    let clusters = engramai::synthesis::cluster::discover_clusters(
        &storage,
        &config,
        Some("ollama/nomic-embed-text"),
    )
    .expect("cluster discovery failed");
    let total_elapsed = total_start.elapsed();

    println!("[{:.3}s] Complete — {} clusters found", total_elapsed.as_secs_f64(), clusters.len());

    if !clusters.is_empty() {
        println!("\n--- Top 10 clusters ---");
        for (i, c) in clusters.iter().take(10).enumerate() {
            println!(
                "  #{}: {} members, quality={:.3}, signal={:?}",
                i + 1,
                c.members.len(),
                c.quality_score,
                c.signals_summary.dominant_signal,
            );
        }

        let sizes: Vec<usize> = clusters.iter().map(|c| c.members.len()).collect();
        let total_clustered: usize = sizes.iter().sum();
        println!("\nMemories in clusters: {} / {} candidates", total_clustered, candidates.len());
        println!(
            "Cluster sizes: min={}, max={}, median={}",
            sizes.iter().min().unwrap(),
            sizes.iter().max().unwrap(),
            {
                let mut s = sizes.clone();
                s.sort();
                s[s.len() / 2]
            },
        );
    }

    println!("\n=== Performance ===");
    println!("N = {}", candidates.len());
    println!("Total time: {:.3}s", total_elapsed.as_secs_f64());
    println!("ISS-001 target: <10s");
    if total_elapsed.as_secs_f64() < 10.0 {
        println!("✅ PASS — {:.3}s < 10s", total_elapsed.as_secs_f64());
    } else {
        println!("❌ FAIL — {:.3}s > 10s", total_elapsed.as_secs_f64());
    }
}
