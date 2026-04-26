//! # IronClaw-Engram: Neuroscience-Grounded Memory for IronClaw Agents
//!
//! IronClaw-Engram is a Rust port of [Engram](https://github.com/tonitangpotato/engram-ai),
//! a memory system for AI agents based on cognitive science models, optimized for
//! integration with [IronClaw](https://github.com/nearai/ironclaw).
//!
//! ## Core Cognitive Models
//!
//! - **ACT-R Activation**: Retrieval based on frequency, recency, and spreading activation
//! - **Memory Chain Model**: Dual-trace consolidation (hippocampus → neocortex)
//! - **Ebbinghaus Forgetting**: Exponential decay with spaced repetition
//! - **Hebbian Learning**: Co-activation forms associative links
//! - **STDP**: Temporal patterns infer causal relationships
//! - **LLM Extraction**: Optional fact extraction via Anthropic/Ollama before storage
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use engramai::{Memory, MemoryType};
//!
//! let mut mem = Memory::new("./agent.db", None)?;
//!
//! // Store memories
//! mem.add(
//!     "potato prefers action over discussion",
//!     MemoryType::Relational,
//!     Some(0.7),
//!     None,
//!     None,
//! )?;
//!
//! // Recall with ACT-R activation
//! let results = mem.recall("what does potato prefer?", 5, None, None)?;
//! for r in results {
//!     println!("[{}] {}", r.confidence_label, r.record.content);
//! }
//!
//! // Consolidate (run "sleep" cycle)
//! mem.consolidate(1.0)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## LLM-Based Extraction
//!
//! Optionally extract structured facts from raw text before storage:
//!
//! ```rust,no_run
//! use engramai::{Memory, MemoryType, OllamaExtractor, AnthropicExtractor};
//!
//! let mut mem = Memory::new("./agent.db", None)?;
//!
//! // Use Ollama for local extraction
//! mem.set_extractor(Box::new(OllamaExtractor::new("llama3.2:3b")));
//!
//! // Or use Anthropic Claude (Haiku recommended for cost)
//! // mem.set_extractor(Box::new(AnthropicExtractor::new("sk-ant-...", false)));
//!
//! // Now add() extracts facts via LLM before storing
//! mem.add(
//!     "我昨天和小明一起吃了火锅，很好吃。他说下周要去上海出差。",
//!     MemoryType::Episodic,
//!     None,
//!     None,
//!     None,
//! )?;
//! // Stores extracted facts like:
//! // - "User ate hotpot yesterday with Xiaoming" (episodic, 0.5)
//! // - "User found the hotpot delicious" (emotional, 0.6)
//! // - "Xiaoming will travel to Shanghai for business next week" (factual, 0.7)
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Configuration Presets
//!
//! ```rust
//! use engramai::MemoryConfig;
//!
//! // Chatbot: slow decay, high replay
//! let config = MemoryConfig::chatbot();
//!
//! // Task agent: fast decay, low replay
//! let config = MemoryConfig::task_agent();
//!
//! // Personal assistant: very slow core decay
//! let config = MemoryConfig::personal_assistant();
//!
//! // Researcher: minimal forgetting
//! let config = MemoryConfig::researcher();
//! ```

pub mod anomaly;
pub mod anthropic_client;
pub mod association;
pub mod bus;
pub mod clustering;
pub mod compiler;
pub mod confidence;
pub mod dimensions;
pub mod enriched;
pub mod entities;
pub mod config;
pub mod embeddings;
pub mod extractor;
pub mod graph;
pub mod resolution;
pub mod hybrid_search;
pub mod interoceptive;
pub mod lifecycle;
pub mod memory;
pub mod merge_types;
pub mod metacognition;
pub mod migration_types;
pub mod models;
pub mod promotion;
pub mod query_classifier;
pub mod session_wm;
pub mod storage;
pub mod store_api;
pub mod synthesis;
pub mod triple;
pub mod triple_extractor;
pub mod type_weights;
pub mod types;
pub mod write_stats;

// Re-export main types
pub use bus::{EmpathyBus, SoulUpdate, HeartbeatUpdate, Drive, HeartbeatTask, Identity, EmpathyTrend, ActionStats, SubscriptionManager, Subscription, Notification, DriveEmbeddings, score_alignment_hybrid};
// Backward-compat aliases
pub use bus::{EmotionalBus, EmotionalTrend};
pub use config::MemoryConfig;
pub use config::TripleConfig;
pub use embeddings::{EmbeddingConfig, EmbeddingProvider, EmbeddingError};
pub use extractor::{MemoryExtractor, ExtractedFact, AnthropicExtractor, AnthropicExtractorConfig, TokenProvider, OllamaExtractor, OllamaExtractorConfig};
pub use type_weights::{TypeWeights, infer_type_weights};
pub use memory::{Memory, SleepReport, is_insight};
pub use write_stats::{
    CountingSink, EventSink, NoopSink, SharedSink, StoreEvent, WriteStats,
};
pub use storage::EmbeddingStats;
pub use storage::EntityRecord;
pub use types::{AclEntry, CrossLink, HebbianLink, MemoryLayer, MergeOutcome, MemoryRecord, MemoryStats, MemoryType, Permission, RecallResult, RecallWithAssociationsResult, SupersessionError, SupersessionInfo, BulkCorrectionResult};

// Re-export new modules
pub use anomaly::{BaselineTracker, Baseline, AnomalyResult};
pub use confidence::{confidence_score, confidence_label, confidence_detail, content_reliability, retrieval_salience, ConfidenceDetail};
pub use hybrid_search::{hybrid_search, adaptive_hybrid_search, reciprocal_rank_fusion, HybridSearchResult, HybridSearchOpts};
pub use session_wm::{ActiveContext, SessionRegistry, SessionRecallResult, CachedScore};

/// Deprecated alias for [`ActiveContext`].
///
/// Renamed in v0.3 to disambiguate from L2 `working_strength` (the r1 trace in
/// the dual-trace consolidation model). `ActiveContext` clearly refers to the
/// session-level active-items buffer (Miller's Law), not the L2 episodic trace
/// strength. See DESIGN-v0.3 review r1 / finding A1.
///
/// Will be removed in v0.4.
#[deprecated(since = "0.3.0", note = "renamed to `ActiveContext` in v0.3; will be removed in v0.4")]
pub type SessionWorkingMemory = ActiveContext;
pub use synthesis::types::{
    SynthesisSettings, SynthesisReport, SynthesisError, SynthesisEngine,
    SynthesisLlmProvider, MemoryCluster, GateDecision, GateResult,
    ProvenanceRecord, ProvenanceChain, UndoSynthesis,
};
pub use triple::{Triple, Predicate, TripleSource};
pub use triple_extractor::{TripleExtractor, AnthropicTripleExtractor, OllamaTripleExtractor};
pub use promotion::PromotionCandidate;
pub use metacognition::{MetaCognitionTracker, MetaCognitionReport, ParameterSuggestion, RecallEvent, SynthesisEvent};
pub use lifecycle::{DecayReport, ForgetReport, AddResult, LifecycleError, PhaseReport, HealthReport, RebalanceReport};
