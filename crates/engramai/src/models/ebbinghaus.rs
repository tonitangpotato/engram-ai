//! Ebbinghaus forgetting curve model.
//!
//! Ebbinghaus forgetting curve:
//!     R(t) = e^(-t/S)
//!
//! Where:
//!     R = retrievability (0-1, probability of successful recall)
//!     t = time since last access
//!     S = stability (determined by repetition, importance, memory type)
//!
//! Stability grows with each successful retrieval (spacing effect):
//!     S_new = S_old * (1 + spacing_factor)

use chrono::{DateTime, Utc};

use crate::types::MemoryRecord;

/// Ebbinghaus retrievability: R = e^(-t/S)
///
/// Stability S is computed from:
/// - Base decay rate (per memory type)
/// - Number of accesses (spaced repetition effect)
/// - Importance (emotional modulation)
///
/// Returns 0-1 probability of successful retrieval.
pub fn retrievability(record: &MemoryRecord, now: DateTime<Utc>) -> f64 {
    // Time since last access (in days)
    let last_access = record.access_times.last().unwrap_or(&record.created_at);
    let t_days = (now - *last_access).num_seconds() as f64 / 86400.0;

    if t_days <= 0.0 {
        return 1.0;
    }

    // Compute stability S
    let s = compute_stability(record);

    (-t_days / s).exp()
}

/// Compute memory stability S.
///
/// Base stability comes from memory type.
/// Each access multiplies stability (spacing effect).
/// Importance further boosts stability.
///
/// S = base_S * (1 + 0.5 * ln(n_accesses + 1)) * (0.5 + importance) * (1 + 0.2 * consolidation_count)
pub fn compute_stability(record: &MemoryRecord) -> f64 {
    // Base stability from memory type (in days)
    let base_decay = record.memory_type.default_decay_rate();
    let base_s = 1.0 / base_decay; // Invert: low decay → high stability

    // Spacing effect: each access increases stability
    let n_accesses = record.access_times.len() as f64;
    let spacing_factor = 1.0 + 0.5 * (n_accesses + 1.0).ln();

    // Importance modulation
    let importance_factor = 0.5 + record.importance; // 0.5x to 1.5x

    // Consolidation bonus
    let consolidation_factor = 1.0 + 0.2 * record.consolidation_count as f64;

    base_s * spacing_factor * importance_factor * consolidation_factor
}

/// Combined strength: Memory Chain trace strengths × Ebbinghaus retrievability.
///
/// This is the final "how alive is this memory" score.
pub fn effective_strength(record: &MemoryRecord, now: DateTime<Utc>) -> f64 {
    let r = retrievability(record, now);
    let trace_strength = record.working_strength + record.core_strength;
    trace_strength * r
}

/// Should this memory be pruned?
///
/// A memory is effectively forgotten when its combined strength
/// drops below threshold.
pub fn should_forget(record: &MemoryRecord, threshold: f64, now: DateTime<Utc>) -> bool {
    if record.pinned {
        return false;
    }
    effective_strength(record, now) < threshold
}
