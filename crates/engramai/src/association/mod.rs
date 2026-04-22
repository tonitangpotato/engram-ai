//! Multi-signal Hebbian association discovery.
//!
//! This module implements write-time association discovery using multiple signals:
//! entity overlap, embedding similarity, and temporal proximity.

pub mod candidate;
pub mod signals;
pub mod former;

pub use candidate::CandidateSelector;
pub use signals::{SignalComputer, SignalScores};
pub use former::LinkFormer;
