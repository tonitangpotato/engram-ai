//! Layer classification — pure function mapping a `MemoryRecord` to its `MemoryLayer`.
//!
//! Per design §9 GOAL-1.14, this is **crate-internal only** — not part of the v0.3
//! public API contract. It is exposed via `pub(crate)` so storage_graph and
//! consolidation can use it without committing to a stable signature.

use crate::types::{MemoryLayer, MemoryRecord};

/// Classify a memory's current layer from its strengths and pinned flag.
///
/// Provisional thresholds per design §9 GOAL-1.14:
/// - `pinned == true`            → Core
/// - `core_strength >= 0.7`      → Core
/// - `working_strength >= 0.6`   → Working
/// - otherwise                   → Archive
///
/// Properties (verified by tests):
/// 1. **Determinism** — same `MemoryRecord` ⇒ same `MemoryLayer` across calls.
/// 2. **Pinned override** — `pinned` always wins, regardless of strengths.
/// 3. **Monotonicity (core)** — raising `core_strength` (holding all else fixed) never demotes.
/// 4. **Coverage** — all three `MemoryLayer` variants are reachable.
#[allow(dead_code)] // Used by storage_graph and consolidation in subsequent GOAL-1.x tasks.
pub(crate) fn classify_layer(r: &MemoryRecord) -> MemoryLayer {
    if r.pinned {
        return MemoryLayer::Core;
    }
    if r.core_strength >= 0.7 {
        return MemoryLayer::Core;
    }
    if r.working_strength >= 0.6 {
        return MemoryLayer::Working;
    }
    MemoryLayer::Archive
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MemoryType;
    use chrono::Utc;

    fn make_record(pinned: bool, core_strength: f64, working_strength: f64) -> MemoryRecord {
        MemoryRecord {
            id: "test0001".into(),
            content: "test".into(),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working, // arbitrary — classify_layer ignores it
            created_at: Utc::now(),
            access_times: vec![],
            working_strength,
            core_strength,
            importance: 0.5,
            pinned,
            consolidation_count: 0,
            last_consolidated: None,
            source: "test".into(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        }
    }

    #[test]
    fn pinned_always_core() {
        let r = make_record(true, 0.0, 0.0);
        assert_eq!(classify_layer(&r), MemoryLayer::Core);
    }

    #[test]
    fn pinned_overrides_low_strengths() {
        let r = make_record(true, 0.0, 0.0);
        assert_eq!(classify_layer(&r), MemoryLayer::Core);
    }

    #[test]
    fn high_core_strength_is_core() {
        let r = make_record(false, 0.8, 0.0);
        assert_eq!(classify_layer(&r), MemoryLayer::Core);
    }

    #[test]
    fn core_threshold_inclusive_at_0_7() {
        let r = make_record(false, 0.7, 0.0);
        assert_eq!(classify_layer(&r), MemoryLayer::Core);
    }

    #[test]
    fn just_below_core_threshold_is_archive_when_working_low() {
        let r = make_record(false, 0.69, 0.0);
        assert_eq!(classify_layer(&r), MemoryLayer::Archive);
    }

    #[test]
    fn working_strength_high_is_working() {
        let r = make_record(false, 0.0, 0.7);
        assert_eq!(classify_layer(&r), MemoryLayer::Working);
    }

    #[test]
    fn working_threshold_inclusive_at_0_6() {
        let r = make_record(false, 0.0, 0.6);
        assert_eq!(classify_layer(&r), MemoryLayer::Working);
    }

    #[test]
    fn just_below_working_threshold_is_archive() {
        let r = make_record(false, 0.0, 0.59);
        assert_eq!(classify_layer(&r), MemoryLayer::Archive);
    }

    #[test]
    fn low_strengths_are_archive() {
        let r = make_record(false, 0.0, 0.0);
        assert_eq!(classify_layer(&r), MemoryLayer::Archive);
    }

    #[test]
    fn all_three_layers_reachable() {
        let core = make_record(true, 0.0, 0.0);
        let working = make_record(false, 0.0, 0.7);
        let archive = make_record(false, 0.0, 0.0);
        assert_eq!(classify_layer(&core), MemoryLayer::Core);
        assert_eq!(classify_layer(&working), MemoryLayer::Working);
        assert_eq!(classify_layer(&archive), MemoryLayer::Archive);
    }

    #[test]
    fn determinism_same_input_same_output() {
        let r = make_record(false, 0.5, 0.5);
        let a = classify_layer(&r);
        let b = classify_layer(&r);
        let c = classify_layer(&r);
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn monotonicity_raising_core_strength_never_demotes() {
        // From Working baseline: raising core_strength to 0.7 promotes to Core.
        let baseline = make_record(false, 0.0, 0.7);
        assert_eq!(classify_layer(&baseline), MemoryLayer::Working);
        let raised = make_record(false, 0.8, 0.7);
        assert_eq!(classify_layer(&raised), MemoryLayer::Core);

        // From Archive baseline: raising core_strength to >=0.7 promotes to Core.
        let baseline_arch = make_record(false, 0.0, 0.0);
        assert_eq!(classify_layer(&baseline_arch), MemoryLayer::Archive);
        let raised_to_core = make_record(false, 0.7, 0.0);
        assert_eq!(classify_layer(&raised_to_core), MemoryLayer::Core);
    }
}
