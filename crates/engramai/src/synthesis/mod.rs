//! Memory synthesis engine — cluster discovery, gate checking, insight generation.
//!
//! Submodules will be activated as they are implemented:
//! - `types`: All type definitions (active)
//! - `cluster`: Cluster discovery algorithm
//! - `gate`: Gate check logic
//! - `insight`: LLM-based insight generation
//! - `provenance`: Provenance tracking and undo
//! - `engine`: Full synthesis engine orchestration

pub mod types;
pub mod cluster;
pub mod gate;
pub mod insight;
pub mod provenance;
pub mod engine;
