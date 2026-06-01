//! Multi-signal Hebbian association discovery.
//!
//! This module implements write-time association discovery using multiple signals:
//! entity overlap, embedding similarity, and temporal proximity.

pub mod candidate;
pub mod former;
pub mod signals;

pub use candidate::CandidateSelector;
pub use former::LinkFormer;
pub use signals::{SignalComputer, SignalScores};
