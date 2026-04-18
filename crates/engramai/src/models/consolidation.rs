//! Memory Chain Consolidation Model (Murre & Chessa, 2011).
//!
//! The brain's dual-system memory transfer, modeled as differential equations:
//!
//! ```text
//! dr1/dt = -mu1 * r1(t)                     (hippocampal trace decays fast)
//! dr2/dt = alpha * r1(t) - mu2 * r2(t)      (neocortical trace grows from hippocampal input, decays slowly)
//! ```
//!
//! Where:
//! - `r1(t)` = working_strength (hippocampal / L3)
//! - `r2(t)` = core_strength (neocortical / L2)
//! - `mu1` = fast decay rate (~0.1/day for working memory)
//! - `mu2` = slow decay rate (~0.005/day for core memory)
//! - `alpha` = consolidation rate (how fast working → core transfer happens)

use chrono::Utc;
use rand::seq::SliceRandom;

use crate::config::MemoryConfig;
use crate::storage::Storage;
use crate::types::{MemoryLayer, MemoryRecord};

/// Apply time-based decay to both memory traces.
///
/// r₁(t+dt) = r₁(t) · e^(-μ₁ · dt)
/// r₂(t+dt) = r₂(t) · e^(-μ₂ · dt)
pub fn apply_decay(record: &mut MemoryRecord, dt_days: f64, mu1: f64, mu2: f64) {
    if record.pinned {
        return;
    }

    record.working_strength *= (-mu1 * dt_days).exp();
    record.core_strength *= (-mu2 * dt_days).exp();
}

/// Run one consolidation step for a single memory.
///
/// This is the "sleep replay" — working trace transfers to core trace.
///
/// dr₂ += α · r₁ · dt   (consolidation transfer)
///
/// Then apply normal decay to both traces.
///
/// Importance modulates consolidation rate (amygdala → hippocampus modulation):
/// effective_alpha = alpha * (0.2 + importance²)
pub fn consolidate_single(record: &mut MemoryRecord, dt_days: f64, config: &MemoryConfig) {
    if record.pinned {
        return;
    }

    // Importance-modulated consolidation
    let effective_alpha = config.alpha * (0.2 + record.importance.powi(2));

    // Transfer from working to core
    let transfer = effective_alpha * record.working_strength * dt_days;
    record.core_strength += transfer;

    // Apply decay
    apply_decay(record, dt_days, config.mu1, config.mu2);

    // Update metadata
    record.consolidation_count += 1;
    record.last_consolidated = Some(Utc::now());
}

/// Run a full consolidation cycle ("sleep").
///
/// 1. Consolidate all working (L3) memories
/// 2. Interleaved replay: also touch some archive (L4) memories
///    (prevents catastrophic forgetting)
/// 3. Promote/demote memories between layers based on strength
///
/// # Arguments
///
/// * `storage` - The storage backend
/// * `dt_days` - Time step in days
/// * `config` - Memory configuration
/// * `namespace` - Optional namespace filter (None = all namespaces, Some("*") = all, Some("ns") = specific)
pub fn run_consolidation_cycle(
    storage: &mut Storage,
    dt_days: f64,
    config: &MemoryConfig,
    namespace: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut all_memories = storage.all_in_namespace(namespace)?;
    let mut rng = rand::thread_rng();

    // Step 1: Consolidate all working memories
    for record in all_memories.iter_mut().filter(|r| r.layer == MemoryLayer::Working) {
        consolidate_single(record, dt_days, config);
    }

    // Step 2: Interleaved replay of archive memories
    let mut archive: Vec<_> = all_memories
        .iter_mut()
        .filter(|r| r.layer == MemoryLayer::Archive)
        .collect();
    
    if !archive.is_empty() {
        let n_replay = ((archive.len() as f64 * config.interleave_ratio).ceil() as usize).max(1);
        archive.shuffle(&mut rng);
        
        for record in archive.iter_mut().take(n_replay) {
            // Replaying an archived memory slightly boosts its core_strength
            record.core_strength += config.replay_boost * (0.5 + record.importance);
            record.consolidation_count += 1;
            record.last_consolidated = Some(Utc::now());
        }
    }

    // Step 3: Decay core memories
    for record in all_memories.iter_mut().filter(|r| r.layer == MemoryLayer::Core) {
        apply_decay(record, dt_days, 0.0, config.mu2); // No working decay for core
    }

    // Step 4: Layer rebalancing
    rebalance_layers(&mut all_memories, config);

    // Write all updates back to storage (in a transaction for atomicity - ISS-001 fix)
    storage.begin_transaction()?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        for record in all_memories {
            storage.update(&record)?;
        }
        Ok(())
    })();
    
    match result {
        Ok(()) => storage.commit_transaction()?,
        Err(e) => {
            let _ = storage.rollback_transaction();
            // Try FTS rebuild in case of corruption
            eprintln!("[engram] Consolidation failed, attempting FTS rebuild: {}", e);
            let _ = storage.rebuild_fts();
            return Err(e);
        }
    }

    Ok(())
}

/// Move memories between layers based on their strength.
///
/// Working → Core: core_strength > promote_threshold
/// Core → Archive: total_strength < demote_threshold
/// Working → Archive: working_strength < archive_threshold
fn rebalance_layers(memories: &mut [MemoryRecord], config: &MemoryConfig) {
    for record in memories {
        let total = record.working_strength + record.core_strength;

        if record.pinned {
            record.layer = MemoryLayer::Core;
            continue;
        }

        match record.layer {
            MemoryLayer::Working => {
                if record.core_strength >= config.promote_threshold {
                    record.layer = MemoryLayer::Core;
                } else if record.working_strength < config.archive_threshold
                    && record.core_strength < config.archive_threshold
                {
                    record.layer = MemoryLayer::Archive;
                }
            }
            MemoryLayer::Core => {
                if total < config.demote_threshold && !record.pinned {
                    record.layer = MemoryLayer::Archive;
                }
            }
            MemoryLayer::Archive => {
                // Archives can be promoted back if replayed enough
                if record.core_strength >= config.promote_threshold {
                    record.layer = MemoryLayer::Core;
                }
            }
        }
    }
}
