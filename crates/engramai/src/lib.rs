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
pub mod bus;
pub mod confidence;
pub mod entities;
pub mod config;
pub mod embeddings;
pub mod extractor;
pub mod hybrid_search;
pub mod memory;
pub mod models;
pub mod session_wm;
pub mod storage;
pub mod synthesis;
pub mod types;

// Re-export main types
pub use bus::{EmotionalBus, SoulUpdate, HeartbeatUpdate, Drive, HeartbeatTask, Identity, EmotionalTrend, ActionStats, SubscriptionManager, Subscription, Notification, DriveEmbeddings, score_alignment_hybrid};
pub use config::MemoryConfig;
pub use embeddings::{EmbeddingConfig, EmbeddingProvider, EmbeddingError};
pub use extractor::{MemoryExtractor, ExtractedFact, AnthropicExtractor, AnthropicExtractorConfig, TokenProvider, OllamaExtractor, OllamaExtractorConfig};
pub use memory::{Memory, SleepReport, is_insight};
pub use storage::EmbeddingStats;
pub use storage::EntityRecord;
pub use types::{AclEntry, CrossLink, HebbianLink, MemoryLayer, MemoryRecord, MemoryStats, MemoryType, Permission, RecallResult, RecallWithAssociationsResult};

// Re-export new modules
pub use anomaly::{BaselineTracker, Baseline, AnomalyResult};
pub use confidence::{confidence_score, confidence_label, confidence_detail, content_reliability, retrieval_salience, ConfidenceDetail};
pub use hybrid_search::{hybrid_search, adaptive_hybrid_search, reciprocal_rank_fusion, HybridSearchResult, HybridSearchOpts};
pub use session_wm::{SessionWorkingMemory, SessionRegistry, SessionRecallResult, CachedScore};
pub use synthesis::types::{
    SynthesisSettings, SynthesisReport, SynthesisError, SynthesisEngine,
    SynthesisLlmProvider, MemoryCluster, GateDecision, GateResult,
    ProvenanceRecord, ProvenanceChain, UndoSynthesis,
};
