//! Interoceptive Layer — The brain-island integration module.
//!
//! Consolidates five internal monitoring subsystems (anomaly, accumulator,
//! feedback, confidence, alignment) into a unified interoceptive system
//! with three layers:
//!
//! 1. **Signal Layer** — raw signals from each subsystem, normalized to
//!    [`InteroceptiveSignal`] format.
//! 2. **Integration Layer** — [`InteroceptiveHub`] aggregates signals into
//!    per-domain [`DomainState`] and global arousal.
//! 3. **Regulation Layer** — generates [`RegulationAction`] suggestions
//!    based on integrated state patterns.
//!
//! Neuroscience grounding:
//! - Craig (2002): interoceptive awareness as three-layer processing
//! - Damasio (1994): somatic markers for rapid situation assessment
//! - Baars (1988): Global Workspace Theory for signal broadcasting
//!
//! See `INTEROCEPTIVE-LAYER.md` for full design rationale.

pub mod types;
pub mod hub;
pub mod regulation;

pub use types::*;
pub use hub::InteroceptiveHub;
pub use regulation::evaluate_with_hub;
