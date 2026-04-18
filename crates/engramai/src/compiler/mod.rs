//! # Knowledge Compiler (KC)
//!
//! Compiles scattered memories into coherent, maintained topic pages.
//! Gated behind the `kc` feature flag.
//!
//! ## Submodules
//!
//! - `types` — Shared type definitions used across all KC subsystems
//!
//! Future submodules (not yet implemented):
//! - `discovery` — Topic candidate discovery from memory clusters
//! - `compilation` — Core compilation pipeline (pure logic + LLM enhancement)
//! - `trigger` — Change detection and recompile triggering
//! - `lifecycle` — Merge, split, archive operations
//! - `feedback` — User feedback processing
//! - `decay` — Knowledge decay and staleness detection
//! - `conflict` — Conflict detection and resolution
//! - `health` — Health reporting and link integrity
//! - `export` — Export/import functionality
//! - `access` — Query and CLI access layer
//! - `privacy` — Privacy level enforcement
//! - `llm` — LLM provider abstraction for KC
//! - `intake` — Document intake and splitting

pub mod api;
pub mod compilation;
pub mod config;
pub mod conflict;
pub mod decay;
pub mod degradation;
pub mod discovery;
pub mod export;
pub mod feedback;
pub mod health;
pub mod import;
pub mod intake;
pub mod llm;
pub mod lock;
pub mod manual_edit;
pub mod privacy;
pub mod storage;
pub mod topic_lifecycle;
pub mod types;
pub mod watcher;

// Re-export all public types for convenience
pub use storage::*;
pub use types::*;
