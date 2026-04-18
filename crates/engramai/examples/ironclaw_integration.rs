//! Example: Integrating engramai with IronClaw
//!
//! Shows how engramai can augment IronClaw's built-in FTS+vector memory
//! with cognitive models (ACT-R activation, Hebbian learning, forgetting).
//!
//! IronClaw's memory: FTS + pgvector → cosine similarity ranking
//! + engramai:         FTS + ACT-R + Hebbian + decay → cognitive ranking
//!
//! Integration approach: engramai runs alongside IronClaw's workspace memory,
//! providing a parallel cognitive memory layer that agents can query via tools.

use engramai::{Memory, MemoryType};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // engramai uses SQLite — lightweight, no PostgreSQL dependency needed.
    // Can run alongside IronClaw's PG-based workspace memory.
    let mut memory = Memory::new("/tmp/ironclaw_engram_demo.db", None)?;

    // === Store memories with cognitive metadata ===

    // Factual memory (high importance, slow decay)
    memory.add(
        "User prefers Rust over Python for systems programming",
        MemoryType::Factual,
        Some(0.9),
        Some("conversation"),
        None,
    )?;

    // Episodic memory (medium importance, normal decay)
    memory.add(
        "Discussed IronClaw WASM sandbox security model with user",
        MemoryType::Episodic,
        Some(0.6),
        Some("conversation"),
        None,
    )?;

    // Procedural memory (how-to knowledge)
    memory.add(
        "To deploy IronClaw: cargo build --release, then ironclaw onboard",
        MemoryType::Procedural,
        Some(0.7),
        Some("conversation"),
        None,
    )?;

    // === Recall with cognitive ranking ===
    // Unlike pure vector similarity, engramai ranks by:
    //   activation = frequency × recency^decay + importance + spreading_activation
    // This means recently-used, frequently-accessed, important memories rank higher.

    let results = memory.recall("user programming language preference", 5, None, None)?;
    for r in &results {
        println!(
            "[activation: {:.3}, confidence: {:.3}] {} (type: {:?})",
            r.activation, r.confidence, r.record.content, r.record.memory_type,
        );
    }

    // === Consolidation ===
    // Run periodically (e.g., in IronClaw's heartbeat/routine).
    // Mimics sleep consolidation: strengthens important memories, decays trivial ones.
    memory.consolidate(7.0)?;

    // === Reward modulation ===
    // When an action based on a memory succeeds, reward recent memories.
    // This strengthens them (dopaminergic feedback).
    memory.reward("positive", 3)?;

    // === Integration with IronClaw ===
    //
    // Option 1: As a native Rust tool in IronClaw's tool registry
    //   - Add `engramai = "0.1"` to IronClaw's Cargo.toml
    //   - Register engram_store, engram_recall, engram_consolidate as tools
    //
    // Option 2: As a CLI tool
    //   - Build engramai as a CLI binary
    //   - IronClaw calls `engram recall "query" --limit 5 --json`
    //
    // Option 3: As a WASM tool (sandboxed)
    //   - Compile to WASM, deploy in IronClaw's sandbox
    //   - Maximum isolation, but adds overhead
    //
    // Recommendation: Option 1 for cognitive memory (trusted, core functionality)
    //                 IronClaw's built-in FTS+vector for workspace search

    let stats = memory.stats()?;
    println!("\n✅ engramai integrated successfully");
    println!("Memories stored: {}", stats.total_memories);
    println!("Layer distribution: {:?}", stats.by_layer);

    // Cleanup
    std::fs::remove_file("/tmp/ironclaw_engram_demo.db").ok();

    Ok(())
}
