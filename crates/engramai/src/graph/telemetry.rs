//! Telemetry emission helpers for the graph layer (§6).
//!
//! The graph layer is a strict one-way **publisher** of OperationalLoad and
//! ResourcePressure signals. It never consumes any signal — that asymmetry is
//! what keeps the dependency graph acyclic (`graph` → `telemetry_bus`, never
//! the reverse).

use std::sync::atomic::{AtomicBool, Ordering};

/// Pluggable telemetry destination. Production wires this to the global telemetry
/// bus; tests use a Vec collector. The graph module never touches global state
/// directly.
pub trait TelemetrySink: Send + Sync {
    fn emit_operational_load(&self, op: &'static str, units: u32);
    fn emit_resource_pressure(&self, subsystem: &'static str, utilization: f64);
}

/// A no-op sink. Useful as a default when telemetry is intentionally disabled
/// (tests that don't care, embedded contexts without a bus).
pub struct NoopSink;

impl TelemetrySink for NoopSink {
    fn emit_operational_load(&self, _op: &'static str, _units: u32) {}
    fn emit_resource_pressure(&self, _subsystem: &'static str, _utilization: f64) {}
}

// Op labels for OperationalLoad — closed set, single source of truth across the graph crate.
pub const OP_INSERT_ENTITY: &str = "insert_entity";
pub const OP_INSERT_EDGE: &str = "insert_edge";
pub const OP_MERGE_ENTITIES: &str = "merge_entities";
pub const OP_DEEP_TRAVERSE: &str = "deep_traverse";
pub const OP_RECORD_EXTRACTION_FAILURE: &str = "record_extraction_failure";
pub const OP_GRAPH_INVARIANT_VIOLATION: &str = "graph_invariant_violation";

// Subsystem labels for ResourcePressure.
pub const SUBSYS_ENTITIES: &str = "entities";
pub const SUBSYS_EDGES: &str = "edges";

// Default watermarks per §6 (calibration deferred to v03-benchmarks).
pub const DEFAULT_ENTITY_PRESSURE_THRESHOLD: u64 = 100_000;
pub const DEFAULT_EDGE_PRESSURE_THRESHOLD: u64 = 500_000;

/// Tracks whether a watermark has been crossed since the last reset, so that
/// ResourcePressure emits exactly once per upward crossing (§6 invariant). The
/// latch is intentionally not auto-resetting — once we've warned that "edges
/// > 500k" we don't re-warn every insert. A future API may expose a manual
/// reset for compaction events.
pub struct WatermarkTracker {
    threshold: u64,
    crossed: AtomicBool,
}

impl WatermarkTracker {
    pub const fn new(threshold: u64) -> Self {
        Self {
            threshold,
            crossed: AtomicBool::new(false),
        }
    }

    /// Returns Some(utilization) on the FIRST call where `current >= threshold`.
    /// Returns None on every subsequent call until `reset()` is invoked.
    /// `utilization` = current / threshold, e.g., 1.05 means 5% over.
    pub fn observe(&self, current: u64) -> Option<f64> {
        if current < self.threshold {
            return None;
        }
        // current >= threshold. Latch the bit; return Some on the transition only.
        match self.crossed.compare_exchange(
            false,
            true,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => Some(current as f64 / self.threshold.max(1) as f64),
            Err(_) => None, // already crossed
        }
    }

    /// Reset the latch (intended for use after compaction reduces the count below threshold).
    pub fn reset(&self) {
        self.crossed.store(false, Ordering::SeqCst);
    }

    pub fn threshold(&self) -> u64 {
        self.threshold
    }
}

/// Emit an OperationalLoad signal for a graph CRUD op. Safe to call with NoopSink.
pub fn emit_operational_load(sink: &dyn TelemetrySink, op: &'static str, units: u32) {
    sink.emit_operational_load(op, units);
}

/// Emit ResourcePressure if and only if the watermark has been newly crossed.
/// Returns true if a signal was emitted.
pub fn emit_pressure_if_crossed(
    sink: &dyn TelemetrySink,
    subsystem: &'static str,
    tracker: &WatermarkTracker,
    current: u64,
) -> bool {
    if let Some(util) = tracker.observe(current) {
        sink.emit_resource_pressure(subsystem, util);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Test sink that records all emissions for assertion.
    struct VecSink {
        ops: Mutex<Vec<(&'static str, u32)>>,
        pressure: Mutex<Vec<(&'static str, f64)>>,
    }

    impl VecSink {
        fn new() -> Self {
            Self {
                ops: Mutex::new(vec![]),
                pressure: Mutex::new(vec![]),
            }
        }
    }

    impl TelemetrySink for VecSink {
        fn emit_operational_load(&self, op: &'static str, units: u32) {
            self.ops.lock().unwrap().push((op, units));
        }
        fn emit_resource_pressure(&self, subsystem: &'static str, utilization: f64) {
            self.pressure.lock().unwrap().push((subsystem, utilization));
        }
    }

    #[test]
    fn noop_sink_does_not_panic() {
        let s = NoopSink;
        s.emit_operational_load(OP_INSERT_ENTITY, 1);
        s.emit_resource_pressure(SUBSYS_ENTITIES, 0.5);
    }

    #[test]
    fn watermark_emits_once_on_crossing() {
        let t = WatermarkTracker::new(100);
        // Below threshold: no emit.
        assert!(t.observe(50).is_none());
        assert!(t.observe(99).is_none());
        // Crossing: emit.
        assert!(t.observe(100).is_some());
        // After cross: silence.
        assert!(t.observe(150).is_none());
        assert!(t.observe(200).is_none());
    }

    #[test]
    fn watermark_reset_re_arms_latch() {
        let t = WatermarkTracker::new(100);
        assert!(t.observe(150).is_some());
        assert!(t.observe(200).is_none());
        t.reset();
        assert!(t.observe(120).is_some());
    }

    #[test]
    fn watermark_threshold_zero_is_safe() {
        // threshold=0 should not divide-by-zero; observe(0) >= 0 is the crossing.
        let t = WatermarkTracker::new(0);
        let r = t.observe(0);
        assert!(r.is_some());
        // Utilization = 0 / max(0, 1) = 0
        assert_eq!(r.unwrap(), 0.0);
    }

    #[test]
    fn emit_operational_load_forwards_to_sink() {
        let s = VecSink::new();
        emit_operational_load(&s, OP_INSERT_EDGE, 3);
        let ops = s.ops.lock().unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0], ("insert_edge", 3));
    }

    #[test]
    fn emit_pressure_if_crossed_returns_true_only_on_crossing() {
        let s = VecSink::new();
        let t = WatermarkTracker::new(100);
        assert!(!emit_pressure_if_crossed(&s, SUBSYS_ENTITIES, &t, 50));
        assert!(emit_pressure_if_crossed(&s, SUBSYS_ENTITIES, &t, 150));
        assert!(!emit_pressure_if_crossed(&s, SUBSYS_ENTITIES, &t, 200));
        let p = s.pressure.lock().unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "entities");
        assert!(p[0].1 >= 1.5 - 0.001 && p[0].1 <= 1.5 + 0.001);
    }

    #[test]
    fn op_constants_are_distinct() {
        let ops = [
            OP_INSERT_ENTITY,
            OP_INSERT_EDGE,
            OP_MERGE_ENTITIES,
            OP_DEEP_TRAVERSE,
            OP_RECORD_EXTRACTION_FAILURE,
            OP_GRAPH_INVARIANT_VIOLATION,
        ];
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for o in ops {
            assert!(seen.insert(o), "duplicate op: {}", o);
        }
    }

    #[test]
    fn default_thresholds_are_documented_values() {
        assert_eq!(DEFAULT_ENTITY_PRESSURE_THRESHOLD, 100_000);
        assert_eq!(DEFAULT_EDGE_PRESSURE_THRESHOLD, 500_000);
    }
}
