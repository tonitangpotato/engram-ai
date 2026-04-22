//! Merge-path public types (MergeWeights, MergeOutcome).
//!
//! `MergeOutcome` exists today in `types.rs`; we re-declare the design
//! shape here for ISS-019 Step 1 and will wire the canonical version
//! in Step 5 (merge_enriched_into). For now this module adds only the
//! new `MergeWeights` type consumed by `Dimensions::union`.
//!
//! See design §5.1 of ISS-019.

use serde::{Deserialize, Serialize};

// Re-export the existing public MergeOutcome so the new write-path API
// has a stable path to use (`store_api::StoreOutcome::Merged` carries
// the id + similarity directly; the richer `MergeOutcome` in types.rs
// stays unchanged and is produced by `merge_memory_into` callers).
pub use crate::types::MergeOutcome;

/// Per-side importance weights fed into `Dimensions::union`.
///
/// Used by the valence merge rule (importance-weighted average) and
/// available to future fields that need a confidence-weighted mix.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MergeWeights {
    /// Importance of the existing (self) side.
    pub existing_importance: f64,
    /// Importance of the incoming (other) side.
    pub incoming_importance: f64,
}

impl MergeWeights {
    /// Equal-weighted merge (used by tests / callers without importance info).
    pub const EQUAL: MergeWeights = MergeWeights {
        existing_importance: 0.5,
        incoming_importance: 0.5,
    };

    pub fn new(existing: f64, incoming: f64) -> Self {
        // Guard against degenerate input. If both are zero / NaN, fall
        // back to equal weighting so union() never divides by zero.
        let e = sanitize(existing);
        let i = sanitize(incoming);
        if e + i == 0.0 {
            Self::EQUAL
        } else {
            Self {
                existing_importance: e,
                incoming_importance: i,
            }
        }
    }

    /// Sum of the two weights. Always > 0 for a value produced by `new`.
    pub fn total(&self) -> f64 {
        self.existing_importance + self.incoming_importance
    }
}

impl Default for MergeWeights {
    fn default() -> Self {
        Self::EQUAL
    }
}

fn sanitize(v: f64) -> f64 {
    if v.is_nan() || v < 0.0 {
        0.0
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_default() {
        let w = MergeWeights::default();
        assert_eq!(w.existing_importance, 0.5);
        assert_eq!(w.incoming_importance, 0.5);
    }

    #[test]
    fn new_preserves_valid_weights() {
        let w = MergeWeights::new(0.9, 0.1);
        assert_eq!(w.existing_importance, 0.9);
        assert_eq!(w.incoming_importance, 0.1);
        assert!((w.total() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn new_sanitizes_nan_and_negatives() {
        let w = MergeWeights::new(f64::NAN, -1.0);
        // Both sanitize to 0 → falls back to EQUAL.
        assert_eq!(w, MergeWeights::EQUAL);
    }

    #[test]
    fn new_zero_sum_falls_back_to_equal() {
        let w = MergeWeights::new(0.0, 0.0);
        assert_eq!(w, MergeWeights::EQUAL);
    }

    #[test]
    fn new_mixed_clamps_negative_to_zero() {
        let w = MergeWeights::new(-0.3, 0.4);
        assert_eq!(w.existing_importance, 0.0);
        assert_eq!(w.incoming_importance, 0.4);
    }
}
