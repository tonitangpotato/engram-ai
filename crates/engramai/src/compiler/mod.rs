//! # Knowledge Compiler (KC)
//!
//! Compiles scattered memories into coherent, maintained topic pages.
//!
//! ## Submodules
//!
//! - `types` — Shared type definitions used across all KC subsystems
//! - `discovery` — Topic candidate discovery from memory clusters
//! - `compilation` — Core compilation pipeline (pure logic + LLM enhancement)
//! - `config` — KC configuration
//! - `conflict` — Conflict detection and resolution
//! - `decay` — Knowledge decay and staleness detection
//! - `degradation` — Capability degradation handling
//! - `export` — Export/import functionality
//! - `feedback` — User feedback processing
//! - `health` — Health reporting and link integrity
//! - `import` — Import pipeline
//! - `intake` — Document intake and splitting
//! - `llm` — LLM provider abstraction for KC
//! - `lock` — Topic locking for concurrent access
//! - `manual_edit` — Manual editing support
//! - `privacy` — Privacy level enforcement
//! - `storage` — Knowledge store abstraction
//! - `topic_lifecycle` — Merge, split, archive operations
//! - `watcher` — Directory watcher for auto-import
//! - `api` — Maintenance and query API

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
