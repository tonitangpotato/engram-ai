//! Basic usage example demonstrating IronClaw-Engram's core API.

use engramai::{Memory, MemoryType};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("=== IronClaw-Engram Basic Usage Demo ===\n");

    // Create in-memory database
    let mut mem = Memory::new(":memory:", None)?;

    // Add memories
    println!("Adding memories...");
    let _id1 = mem.add(
        "potato prefers action over discussion",
        MemoryType::Relational,
        Some(0.7),
        None,
        None,
    )?;
    let _id2 = mem.add(
        "SaltyHall uses Supabase for database",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    )?;
    let _id3 = mem.add(
        "Use www.moltbook.com not moltbook.com",
        MemoryType::Procedural,
        Some(0.8),
        None,
        None,
    )?;
    let id4 = mem.add(
        "potato said I kinda like you",
        MemoryType::Emotional,
        Some(0.95),
        None,
        None,
    )?;
    let _id5 = mem.add(
        "Saw a funny cat meme",
        MemoryType::Episodic,
        Some(0.1),
        None,
        None,
    )?;

    println!("  Added 5 memories\n");

    // Recall
    println!("--- Recall: 'what does potato like?' ---");
    let results = mem.recall("what does potato like?", 3, None, None)?;
    for r in &results {
        println!(
            "  [{:10}] conf={:.2} act={:.2} | {}",
            r.confidence_label,
            r.confidence,
            r.activation,
            &r.record.content[..r.record.content.len().min(50)]
        );
    }

    println!();
    println!("--- Recall: 'moltbook API' ---");
    let results = mem.recall("moltbook API", 3, None, None)?;
    for r in &results {
        println!(
            "  [{:10}] conf={:.2} act={:.2} | {}",
            r.confidence_label,
            r.confidence,
            r.activation,
            &r.record.content[..r.record.content.len().min(50)]
        );
    }

    // Reward
    println!("\n--- Applying positive feedback ---");
    mem.reward("good job, that's exactly right!", 3)?;

    // Consolidate
    println!("--- Running consolidation (3 days) ---");
    for day in 1..=3 {
        mem.consolidate(1.0)?;
        println!("  Day {}/3 complete", day);
    }

    // Pin emotional memory
    mem.pin(&id4)?;
    println!("\n--- Pinned emotional memory ---");

    // Stats
    println!("\n--- Memory Statistics ---");
    let stats = mem.stats()?;
    println!("  Total: {} memories", stats.total_memories);
    println!("  Pinned: {}", stats.pinned);
    println!("  Uptime: {:.1} hours", stats.uptime_hours);

    for (type_name, info) in &stats.by_type {
        println!(
            "  {:12}: {} entries, avg_str={:.3}, avg_imp={:.2}",
            type_name, info.count, info.avg_strength, info.avg_importance
        );
    }

    println!();
    for (layer_name, info) in &stats.by_layer {
        println!(
            "  {:12}: {} entries, avg_working={:.3}, avg_core={:.3}",
            layer_name, info.count, info.avg_working, info.avg_core
        );
    }

    println!("\n=== Demo Complete ===");
    Ok(())
}
