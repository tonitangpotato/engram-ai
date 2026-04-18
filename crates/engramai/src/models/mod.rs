//! Cognitive models for memory dynamics.

pub mod actr;
pub mod consolidation;
pub mod ebbinghaus;
pub mod hebbian;

pub use actr::{base_level_activation, retrieval_activation};
pub use consolidation::{apply_decay, consolidate_single, run_consolidation_cycle};
pub use ebbinghaus::{compute_stability, effective_strength, retrievability};
pub use hebbian::{record_coactivation, record_coactivation_ns};
