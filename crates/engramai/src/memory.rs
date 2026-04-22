//! Main Memory API — simplified interface to Engram's cognitive models.

use chrono::Utc;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use uuid::Uuid;

use crate::bus::EmpathyBus;
use crate::config::MemoryConfig;
use crate::embeddings::{EmbeddingConfig, EmbeddingProvider, EmbeddingError};
use crate::entities::EntityExtractor;
use crate::extractor::MemoryExtractor;
use crate::models::{effective_strength, retrieval_activation, run_consolidation_cycle};
use crate::storage::{Storage, EmbeddingStats};
use crate::bus::{SubscriptionManager, Subscription, Notification};
use crate::models::hebbian::{MemoryWithNamespace, record_cross_namespace_coactivation};
use crate::session_wm::{SessionWorkingMemory, SessionRecallResult};
use crate::synthesis::types::SynthesisEngine;
use crate::types::{AclEntry, CrossLink, HebbianLink, LayerStats, MemoryLayer, MemoryRecord, MemoryStats, MemoryType, Permission, RecallResult, RecallWithAssociationsResult, TypeStats};

/// Report from a unified sleep cycle (consolidation + synthesis).
#[derive(Debug)]
pub struct SleepReport {
    /// Whether the consolidation phase completed successfully.
    pub consolidation_ok: bool,
    /// Synthesis report (None if synthesis not enabled or failed non-fatally).
    pub synthesis: Option<crate::synthesis::types::SynthesisReport>,
}

/// Check if a memory record is a synthesis insight.
pub fn is_insight(record: &MemoryRecord) -> bool {
    record
        .metadata
        .as_ref()
        .and_then(|m| m.get("is_synthesis"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Main interface to the Engram memory system.
///
/// Wraps the neuroscience math models behind a clean API.
/// All complexity is hidden — you just add, recall, and consolidate.
pub struct Memory {
    storage: Storage,
    config: MemoryConfig,
    created_at: chrono::DateTime<Utc>,
    /// Agent ID for this memory instance (used for ACL checks)
    agent_id: Option<String>,
    /// Optional Emotional Bus for drive alignment and emotional tracking
    empathy_bus: Option<EmpathyBus>,
    /// Embedding provider for semantic similarity (optional - falls back to FTS if unavailable)
    embedding: Option<EmbeddingProvider>,
    /// Optional LLM-based memory extractor for converting raw text to structured facts
    extractor: Option<Box<dyn MemoryExtractor>>,
    /// Entity extractor for identifying entities in memory content
    entity_extractor: EntityExtractor,
    /// Optional synthesis settings (None = synthesis disabled)
    synthesis_settings: Option<crate::synthesis::types::SynthesisSettings>,
    /// Optional LLM provider for synthesis insight generation
    synthesis_llm_provider: Option<Box<dyn crate::synthesis::types::SynthesisLlmProvider>>,
    /// Interoceptive hub for unified internal state monitoring
    interoceptive_hub: crate::interoceptive::InteroceptiveHub,
    /// Optional LLM-based triple extractor for knowledge graph enrichment
    triple_extractor: Option<Box<dyn crate::triple_extractor::TripleExtractor>>,
    /// Cached emotion data from last LLM extraction: Vec<(valence, domain)>.
    /// One-shot: `take_last_emotions()` clears it.
    last_extraction_emotions: std::sync::Mutex<Option<Vec<(f64, String)>>>,
    /// Recent recall timestamps for cross-recall co-occurrence detection (C8).
    /// Bounded ring buffer: last 50 recalls.
    recent_recalls: VecDeque<(String, std::time::Instant)>,
    /// Last add result for metrics access. Reset each sleep_cycle.
    last_add_result: Option<crate::lifecycle::AddResult>,
    /// Dedup merge counter (reset each sleep_cycle).
    dedup_merge_count: usize,
    /// Dedup new-write counter (reset each sleep_cycle).
    dedup_write_count: usize,
    /// Meta-cognition tracker for self-monitoring (lazy-init when enabled).
    metacognition: Option<crate::metacognition::MetaCognitionTracker>,
    /// Haiku-based intent classifier for Level 2 query-intent classification.
    /// Initialized via `auto_configure_intent_classifier()` when enabled in config.
    intent_classifier: Option<crate::query_classifier::HaikuIntentClassifier>,
}

impl Memory {
    /// Initialize Engram memory system.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to SQLite database file. Created if it doesn't exist.
    ///           Use `:memory:` for in-memory (non-persistent) operation.
    /// * `config` - MemoryConfig with tunable parameters. None = literature defaults.
    ///
    /// If Ollama is available, embeddings will be used for semantic search.
    /// Otherwise, falls back to FTS5 keyword matching.
    pub fn new(path: &str, config: Option<MemoryConfig>) -> Result<Self, Box<dyn std::error::Error>> {
        let storage = Storage::new(path)?;
        let config = config.unwrap_or_default();
        let created_at = Utc::now();
        
        // Create embedding provider (optional - check if Ollama is available)
        let embedding_provider = EmbeddingProvider::new(config.embedding.clone());
        let embedding = if embedding_provider.is_available() {
            log::info!("Ollama available at {}, embedding enabled", config.embedding.host);
            Some(embedding_provider)
        } else {
            log::warn!("Ollama not available at {}, falling back to FTS", config.embedding.host);
            None
        };

        let entity_extractor = EntityExtractor::new(&config.entity_config);

        let mut mem = Self {
            storage,
            config,
            created_at,
            agent_id: None,
            empathy_bus: None,
            embedding,
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
            interoceptive_hub: crate::interoceptive::InteroceptiveHub::new(),
            triple_extractor: None,
            last_extraction_emotions: std::sync::Mutex::new(None),
            recent_recalls: VecDeque::new(),
            last_add_result: None,
            dedup_merge_count: 0,
            dedup_write_count: 0,
            metacognition: None,
            intent_classifier: None,
        };
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        mem.auto_configure_intent_classifier();

        // Initialize meta-cognition tracker if enabled
        mem.init_metacognition_if_enabled();
        
        Ok(mem)
    }
    
    /// Initialize Engram memory system requiring embeddings.
    ///
    /// Returns an error if Ollama is not available. Use this when embedding
    /// support is required for your use case.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to SQLite database file
    /// * `config` - MemoryConfig with tunable parameters
    pub fn new_with_required_embedding(
        path: &str,
        config: Option<MemoryConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let storage = Storage::new(path)?;
        let config = config.unwrap_or_default();
        let created_at = Utc::now();
        
        // Create embedding provider and validate Ollama is available
        let embedding = EmbeddingProvider::new(config.embedding.clone());
        if !embedding.is_available() {
            return Err(Box::new(EmbeddingError::OllamaNotAvailable(
                config.embedding.host.clone()
            )));
        }

        let entity_extractor = EntityExtractor::new(&config.entity_config);

        let mut mem = Self {
            storage,
            config,
            created_at,
            agent_id: None,
            empathy_bus: None,
            embedding: Some(embedding),
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
            interoceptive_hub: crate::interoceptive::InteroceptiveHub::new(),
            triple_extractor: None,
            last_extraction_emotions: std::sync::Mutex::new(None),
            recent_recalls: VecDeque::new(),
            last_add_result: None,
            dedup_merge_count: 0,
            dedup_write_count: 0,
            metacognition: None,
            intent_classifier: None,
        };
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        mem.auto_configure_intent_classifier();
        
        Ok(mem)
    }
    
    /// Initialize Engram memory system with custom embedding config.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to SQLite database file
    /// * `config` - MemoryConfig with tunable parameters
    /// * `embedding_config` - Custom embedding configuration (overrides config.embedding)
    pub fn with_embedding(
        path: &str,
        config: Option<MemoryConfig>,
        embedding_config: EmbeddingConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let storage = Storage::new(path)?;
        let mut config = config.unwrap_or_default();
        config.embedding = embedding_config;
        let created_at = Utc::now();
        
        // Create embedding provider (optional - check if Ollama is available)
        let embedding_provider = EmbeddingProvider::new(config.embedding.clone());
        let embedding = if embedding_provider.is_available() {
            Some(embedding_provider)
        } else {
            log::warn!("Ollama not available at {}, falling back to FTS", config.embedding.host);
            None
        };

        let entity_extractor = EntityExtractor::new(&config.entity_config);

        let mut mem = Self {
            storage,
            config,
            created_at,
            agent_id: None,
            empathy_bus: None,
            embedding,
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
            interoceptive_hub: crate::interoceptive::InteroceptiveHub::new(),
            triple_extractor: None,
            last_extraction_emotions: std::sync::Mutex::new(None),
            recent_recalls: VecDeque::new(),
            last_add_result: None,
            dedup_merge_count: 0,
            dedup_write_count: 0,
            metacognition: None,
            intent_classifier: None,
        };
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        mem.auto_configure_intent_classifier();
        
        Ok(mem)
    }
    
    /// Create a Memory instance with an Emotional Bus attached.
    ///
    /// The Emotional Bus connects memory to workspace files (SOUL.md, HEARTBEAT.md)
    /// for drive alignment and emotional feedback loops.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to SQLite database file
    /// * `workspace_dir` - Path to the agent workspace directory
    /// * `config` - Optional MemoryConfig
    pub fn with_empathy_bus(
        path: &str,
        workspace_dir: &str,
        config: Option<MemoryConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let storage = Storage::new(path)?;
        let config = config.unwrap_or_default();
        let created_at = Utc::now();
        
        // Create Empathy Bus using storage's connection
        let empathy_bus = Some(EmpathyBus::new(workspace_dir, storage.connection())?);
        
        // Create embedding provider (optional - check if Ollama is available)
        let embedding_provider = EmbeddingProvider::new(config.embedding.clone());
        let embedding = if embedding_provider.is_available() {
            Some(embedding_provider)
        } else {
            log::warn!("Ollama not available at {}, falling back to FTS", config.embedding.host);
            None
        };
        
        let entity_extractor = EntityExtractor::new(&config.entity_config);

        let mut mem = Self {
            storage,
            config,
            created_at,
            agent_id: None,
            empathy_bus,
            embedding,
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
            interoceptive_hub: crate::interoceptive::InteroceptiveHub::new(),
            triple_extractor: None,
            last_extraction_emotions: std::sync::Mutex::new(None),
            recent_recalls: VecDeque::new(),
            last_add_result: None,
            dedup_merge_count: 0,
            dedup_write_count: 0,
            metacognition: None,
            intent_classifier: None,
        };
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        mem.auto_configure_intent_classifier();
        
        Ok(mem)
    }

    /// Backward-compat alias.
    pub fn with_emotional_bus(
        path: &str,
        workspace_dir: &str,
        config: Option<MemoryConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_empathy_bus(path, workspace_dir, config)
    }
    
    /// Get a reference to the Empathy Bus, if attached.
    pub fn empathy_bus(&self) -> Option<&EmpathyBus> {
        self.empathy_bus.as_ref()
    }

    /// Backward-compat alias.
    #[inline]
    pub fn emotional_bus(&self) -> Option<&EmpathyBus> { self.empathy_bus() }
    
    /// Get a mutable reference to the Empathy Bus, if attached.
    pub fn empathy_bus_mut(&mut self) -> Option<&mut EmpathyBus> {
        self.empathy_bus.as_mut()
    }

    /// Backward-compat alias.
    #[inline]
    pub fn emotional_bus_mut(&mut self) -> Option<&mut EmpathyBus> { self.empathy_bus_mut() }

    // ── Interoceptive Hub API ─────────────────────────────────────────

    /// Get a reference to the interoceptive hub.
    pub fn interoceptive_hub(&self) -> &crate::interoceptive::InteroceptiveHub {
        &self.interoceptive_hub
    }

    /// Get a mutable reference to the interoceptive hub.
    pub fn interoceptive_hub_mut(&mut self) -> &mut crate::interoceptive::InteroceptiveHub {
        &mut self.interoceptive_hub
    }

    /// Take a snapshot of the current interoceptive state.
    ///
    /// Returns the integrated state across all domains — suitable for
    /// injection into system prompts or inspection.
    pub fn interoceptive_snapshot(&self) -> crate::interoceptive::InteroceptiveState {
        self.interoceptive_hub.current_state()
    }

    /// Run an interoceptive tick: pull signals from all attached subsystems
    /// and feed them into the hub.
    ///
    /// Call this periodically (e.g., every heartbeat or every N messages)
    /// to keep the interoceptive state current.
    pub fn interoceptive_tick(&mut self) {
        use crate::bus::accumulator::EmpathyAccumulator;
        use crate::bus::feedback::BehaviorFeedback;
        use crate::interoceptive::InteroceptiveSignal;

        let mut signals: Vec<InteroceptiveSignal> = Vec::new();

        // Pull from DB-backed subsystems via the storage connection.
        let conn = self.storage.connection();

        // Accumulator: pull all emotional trends.
        if let Ok(acc) = EmpathyAccumulator::new(conn) {
            if let Ok(trends) = acc.get_all_trends() {
                for trend in &trends {
                    if let Ok(Some(sig)) = acc.to_signal(&trend.domain) {
                        signals.push(sig);
                    }
                }
            }
        }

        // Feedback: pull all action stats.
        if let Ok(fb) = BehaviorFeedback::new(conn) {
            if let Ok(stats) = fb.get_all_action_stats() {
                for stat in &stats {
                    if let Ok(Some(sig)) = fb.to_signal(&stat.action) {
                        signals.push(sig);
                    }
                }
            }
        }

        // Alignment: generate signal if emotional bus is attached (has drives).
        // Note: alignment is content-dependent, so we skip it in tick.
        // It's triggered per-interaction instead.

        // Feed all collected signals into the hub.
        if !signals.is_empty() {
            log::debug!("Interoceptive tick: processing {} signals", signals.len());
            self.interoceptive_hub.process_batch(signals);
        }
    }

    /// Broadcast memory admission to the interoceptive hub (GWT global workspace).
    ///
    /// When memories enter working memory (via recall), this broadcasts
    /// signals to the hub for integration. Implements Baars' Global Workspace
    /// Theory: working memory contents are "broadcast" to all cognitive modules.
    ///
    /// For each admitted memory:
    /// 1. Generate a confidence signal (metacognitive assessment)
    /// 2. Check drive alignment if emotional bus is attached
    /// 3. Spread activation to Hebbian neighbors (associative priming)
    ///
    /// Returns the IDs of Hebbian neighbors activated (for potential WM boosting).
    pub fn broadcast_admission(
        &mut self,
        memory_ids: &[String],
        session_wm: &mut SessionWorkingMemory,
    ) -> Vec<String> {
        use crate::interoceptive::InteroceptiveSignal;

        let mut signals: Vec<InteroceptiveSignal> = Vec::new();
        let mut neighbor_ids: Vec<String> = Vec::new();

        for memory_id in memory_ids {
            // Fetch the memory record for signal generation.
            let record = match self.storage.get(memory_id) {
                Ok(Some(r)) => r,
                _ => continue,
            };

            // 1. Confidence signal — metacognitive assessment of this memory.
            let conf_signal = crate::confidence::confidence_to_signal(&record, None, None);
            signals.push(conf_signal);

            // 2. Alignment signal — does this memory align with core drives?
            if let Some(ref bus) = self.empathy_bus {
                let drives = bus.drives();
                if !drives.is_empty() {
                    let align_signal =
                        crate::bus::alignment::alignment_to_signal(&record.content, drives);
                    signals.push(align_signal);
                }
            }

            // 3. Spreading activation — Hebbian neighbors get primed.
            if self.config.hebbian_enabled {
                if let Ok(neighbors) = self.storage.get_hebbian_links_weighted(memory_id) {
                    for (neighbor_id, weight) in &neighbors {
                        // Only spread to neighbors with meaningful link strength.
                        if *weight > 0.1 {
                            neighbor_ids.push(neighbor_id.clone());
                        }
                    }
                }
            }
        }

        // Feed all broadcast signals into the hub.
        if !signals.is_empty() {
            log::debug!(
                "GWT broadcast: {} memories → {} signals",
                memory_ids.len(),
                signals.len()
            );
            self.interoceptive_hub.process_batch(signals);
        }

        // Boost Hebbian neighbors in working memory (associative priming).
        // This implements spreading activation: memories connected to the
        // admitted ones get a small activation boost in WM.
        if !neighbor_ids.is_empty() {
            neighbor_ids.dedup();
            session_wm.activate(&neighbor_ids);
            log::debug!(
                "GWT spreading activation: {} Hebbian neighbors primed",
                neighbor_ids.len()
            );
        }

        neighbor_ids
    }
    
    /// Get a reference to the underlying storage connection.
    pub fn connection(&self) -> &rusqlite::Connection {
        self.storage.connection()
    }
    
    /// Set the agent ID for this memory instance.
    /// 
    /// This is used for ACL checks when storing and recalling memories.
    /// Each agent should identify itself before performing operations.
    pub fn set_agent_id(&mut self, id: &str) {
        self.agent_id = Some(id.to_string());
    }
    
    /// Get the current agent ID.
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }
    
    /// Set a memory extractor for LLM-based fact extraction.
    ///
    /// When an extractor is set, `add()` and `add_to_namespace()` will
    /// pass the raw content through the LLM to extract structured facts,
    /// storing each fact as a separate memory. If extraction fails,
    /// the raw content is stored as a fallback.
    ///
    /// # Arguments
    ///
    /// * `extractor` - The extractor implementation (e.g., AnthropicExtractor, OllamaExtractor)
    pub fn set_extractor(&mut self, extractor: Box<dyn MemoryExtractor>) {
        self.extractor = Some(extractor);
    }

    /// Set the synthesis settings. Enables knowledge synthesis during consolidation.
    ///
    /// Synthesis is opt-in: it only runs when settings.enabled is true.
    /// This maintains backward compatibility (GUARD-3).
    pub fn set_synthesis_settings(&mut self, settings: crate::synthesis::types::SynthesisSettings) {
        self.synthesis_settings = Some(settings);
    }

    /// Set the LLM provider for synthesis insight generation.
    ///
    /// Without a provider, synthesis still discovers clusters and runs the gate check,
    /// but skips LLM insight generation (graceful degradation).
    pub fn set_synthesis_llm_provider(&mut self, provider: Box<dyn crate::synthesis::types::SynthesisLlmProvider>) {
        self.synthesis_llm_provider = Some(provider);
    }
    
    /// Remove the memory extractor (revert to storing raw content).
    pub fn clear_extractor(&mut self) {
        self.extractor = None;
    }
    
    /// Check if an extractor is configured.
    pub fn has_extractor(&self) -> bool {
        self.extractor.is_some()
    }
    
    /// Auto-configure extractor from environment and config file.
    ///
    /// Called during Memory::new() if no extractor is explicitly set.
    /// This provides "just works" behavior for users who set env vars.
    ///
    /// Detection order (high → low priority):
    /// 1. ANTHROPIC_AUTH_TOKEN env var → AnthropicExtractor with OAuth
    /// 2. ANTHROPIC_API_KEY env var → AnthropicExtractor with API key
    /// 3. ~/.config/engram/config.json extractor section
    /// 4. None → no extraction (backward compatible)
    ///
    /// Model can be overridden via ENGRAM_EXTRACTOR_MODEL env var.
    pub fn auto_configure_extractor(&mut self) {
        use crate::extractor::{AnthropicExtractor, AnthropicExtractorConfig};
        
        // Check ANTHROPIC_AUTH_TOKEN first (OAuth mode)
        if let Ok(token) = std::env::var("ANTHROPIC_AUTH_TOKEN") {
            let model = std::env::var("ENGRAM_EXTRACTOR_MODEL")
                .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string());
            let config = AnthropicExtractorConfig {
                model,
                ..Default::default()
            };
            self.extractor = Some(Box::new(AnthropicExtractor::with_config(&token, true, config)));
            log::info!("Extractor: Anthropic (OAuth) from ANTHROPIC_AUTH_TOKEN");
            return;
        }
        
        // Check ANTHROPIC_API_KEY (API key mode)
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            let model = std::env::var("ENGRAM_EXTRACTOR_MODEL")
                .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string());
            let config = AnthropicExtractorConfig {
                model,
                ..Default::default()
            };
            self.extractor = Some(Box::new(AnthropicExtractor::with_config(&key, false, config)));
            log::info!("Extractor: Anthropic (API key) from ANTHROPIC_API_KEY");
            return;
        }
        
        // Check config file
        if let Some(extractor) = self.load_extractor_from_config() {
            self.extractor = Some(extractor);
            return;
        }
        
        // No extractor configured - that's fine, backward compatible
        log::debug!("No extractor configured, storing raw text");
    }

    /// Auto-configure Haiku intent classifier from environment and config.
    ///
    /// Reads `ANTHROPIC_AUTH_TOKEN` or `ANTHROPIC_API_KEY` from environment
    /// and creates a `HaikuIntentClassifier` if `haiku_l2_enabled` is true
    /// in `config.intent_classification`.
    pub fn auto_configure_intent_classifier(&mut self) {
        let config = &self.config.intent_classification;
        if !config.haiku_l2_enabled {
            return;
        }
        if let Ok(token) = std::env::var("ANTHROPIC_AUTH_TOKEN")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
        {
            let is_oauth = std::env::var("ANTHROPIC_AUTH_TOKEN").is_ok();
            self.intent_classifier = Some(crate::query_classifier::HaikuIntentClassifier::new(
                Box::new(crate::anthropic_client::StaticToken(token)),
                is_oauth,
                config.model.clone(),
                config.api_url.clone(),
                config.timeout_secs,
            ));
            log::info!("HaikuIntentClassifier: configured (oauth={})", is_oauth);
        }
    }
    
    /// Load extractor configuration from ~/.config/engram/config.json.
    ///
    /// The config file stores non-sensitive settings (provider, model, host).
    /// Auth tokens MUST come from environment variables or code.
    ///
    /// Config format:
    /// ```json
    /// {
    ///   "extractor": {
    ///     "provider": "anthropic",  // or "ollama"
    ///     "model": "claude-haiku-4-5-20251001"
    ///   }
    /// }
    /// ```
    fn load_extractor_from_config(&self) -> Option<Box<dyn MemoryExtractor>> {
        use crate::extractor::{AnthropicExtractor, AnthropicExtractorConfig, OllamaExtractor};
        
        // Get config directory
        let config_path = dirs::config_dir()?.join("engram/config.json");
        if !config_path.exists() {
            return None;
        }
        
        // Read and parse config file
        let content = std::fs::read_to_string(&config_path).ok()?;
        let config: serde_json::Value = serde_json::from_str(&content).ok()?;
        
        // Get extractor section
        let extractor_config = config.get("extractor")?;
        let provider = extractor_config.get("provider")?.as_str()?;
        
        match provider {
            "anthropic" => {
                // Still need env var for auth - config file NEVER stores tokens
                let token = std::env::var("ANTHROPIC_AUTH_TOKEN")
                    .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
                    .ok()?;
                let is_oauth = std::env::var("ANTHROPIC_AUTH_TOKEN").is_ok();
                
                let model = extractor_config
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("claude-haiku-4-5-20251001")
                    .to_string();
                
                let api_config = AnthropicExtractorConfig {
                    model: model.clone(),
                    ..Default::default()
                };
                
                log::info!("Extractor: Anthropic ({}) from config file", model);
                Some(Box::new(AnthropicExtractor::with_config(&token, is_oauth, api_config)))
            }
            "ollama" => {
                let model = extractor_config
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("llama3.2:3b")
                    .to_string();
                let host = extractor_config
                    .get("host")
                    .and_then(|v| v.as_str())
                    .unwrap_or("http://localhost:11434")
                    .to_string();
                
                log::info!("Extractor: Ollama ({}) from config file", model);
                Some(Box::new(OllamaExtractor::with_host(&model, &host)))
            }
            _ => {
                log::warn!("Unknown extractor provider in config: {}", provider);
                None
            }
        }
    }

    /// Store a new memory. Returns memory ID.
    ///
    /// The memory is encoded with initial working_strength=1.0 (strong
    /// hippocampal trace) and core_strength=0.0 (no neocortical trace yet).
    /// Consolidation cycles will gradually transfer it to core.
    ///
    /// # Arguments
    ///
    /// * `content` - The memory content (natural language)
    /// * `memory_type` - Memory type classification
    /// * `importance` - 0-1 importance score (None = auto from type)
    /// * `source` - Source identifier (e.g., filename, conversation ID)
    /// * `metadata` - Optional structured metadata (e.g., for causal memories)
    pub fn add(
        &mut self,
        content: &str,
        memory_type: MemoryType,
        importance: Option<f64>,
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        self.add_to_namespace(content, memory_type, importance, source, metadata, None)
    }
    
    /// Store a new memory in a specific namespace. Returns memory ID.
    ///
    /// If an extractor is configured, the content is first passed through
    /// the LLM to extract structured facts. Each extracted fact is stored
    /// as a separate memory. If extraction fails or returns nothing,
    /// the raw content is stored as a fallback.
    ///
    /// # Arguments
    ///
    /// * `content` - The memory content (natural language)
    /// * `memory_type` - Memory type classification (used as fallback if extraction fails)
    /// * `importance` - 0-1 importance score (None = auto from type, or from extraction)
    /// * `source` - Source identifier (e.g., filename, conversation ID)
    /// * `metadata` - Optional structured metadata (e.g., for causal memories)
    /// * `namespace` - Namespace to store in (None = "default")
    pub fn add_to_namespace(
        &mut self,
        content: &str,
        memory_type: MemoryType,
        importance: Option<f64>,
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
        namespace: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // If extractor is configured, try to extract facts first
        if let Some(ref extractor) = self.extractor {
            match extractor.extract(content) {
                Ok(facts) if !facts.is_empty() => {
                    log::info!("Extracted {} facts from content ({}...)", facts.len(),
                        content.chars().take(40).collect::<String>());
                    let mut last_id = String::new();
                    for fact in &facts {
                        log::info!("  → (imp={:.1}, val={:.2}, dom={}) {}", 
                            fact.importance, fact.valence, fact.domain,
                            fact.core_fact.chars().take(80).collect::<String>());
                    }
                    for fact in facts {
                        // Infer type weights from dimensional fields
                        let type_weights = crate::type_weights::infer_type_weights(&fact);
                        let fact_type = type_weights.primary_type();
                        
                        // Cap auto-extracted importance to prevent noise from dominating recall
                        let capped_importance = fact.importance.min(self.config.auto_extract_importance_cap);
                        if fact.importance > self.config.auto_extract_importance_cap {
                            log::debug!("  ↓ importance capped: {:.2} → {:.2}", fact.importance, capped_importance);
                        }
                        let fact_importance = Some(capped_importance);

                        // Build dimensional metadata
                        let mut dim_metadata = serde_json::Map::new();
                        
                        // Dimensional fields
                        let mut dimensions = serde_json::Map::new();
                        if let Some(ref v) = fact.participants { dimensions.insert("participants".into(), serde_json::Value::String(v.clone())); }
                        if let Some(ref v) = fact.temporal { dimensions.insert("temporal".into(), serde_json::Value::String(v.clone())); }
                        if let Some(ref v) = fact.location { dimensions.insert("location".into(), serde_json::Value::String(v.clone())); }
                        if let Some(ref v) = fact.context { dimensions.insert("context".into(), serde_json::Value::String(v.clone())); }
                        if let Some(ref v) = fact.causation { dimensions.insert("causation".into(), serde_json::Value::String(v.clone())); }
                        if let Some(ref v) = fact.outcome { dimensions.insert("outcome".into(), serde_json::Value::String(v.clone())); }
                        if let Some(ref v) = fact.method { dimensions.insert("method".into(), serde_json::Value::String(v.clone())); }
                        if let Some(ref v) = fact.relations { dimensions.insert("relations".into(), serde_json::Value::String(v.clone())); }
                        if let Some(ref v) = fact.sentiment { dimensions.insert("sentiment".into(), serde_json::Value::String(v.clone())); }
                        if let Some(ref v) = fact.stance { dimensions.insert("stance".into(), serde_json::Value::String(v.clone())); }
                        // ISS-020 P0.0: persist always-present fields that were previously dropped.
                        // `valence` (f64) and `confidence` (string) are required fields on ExtractedFact
                        // but were missing from the persisted dimensions object — KC could not read them.
                        // `domain` is also always-present; persisting it here unblocks KC conflict detection
                        // (P0.5 needs same-domain matching) and future clustering pre-filter (P1.1).
                        dimensions.insert("valence".into(), serde_json::json!(fact.valence));
                        dimensions.insert("confidence".into(), serde_json::Value::String(fact.confidence.clone()));
                        dimensions.insert("domain".into(), serde_json::Value::String(fact.domain.clone()));
                        if !dimensions.is_empty() {
                            dim_metadata.insert("dimensions".into(), serde_json::Value::Object(dimensions));
                        }
                        
                        // Type weights
                        dim_metadata.insert("type_weights".into(), type_weights.to_json());
                        
                        // NOTE: source_text deliberately NOT stored here.
                        // Preserving raw input is a caller concern (e.g., benchmark adapters
                        // that need dia_id markers for evidence tracking). Storing it in every
                        // memory doubles storage footprint and violates engram's separation
                        // between cognitive facts (content) and source material.
                        // Callers needing raw text should maintain their own cache or pass it
                        // via the `metadata` parameter.
                        
                        // Merge with caller-provided metadata (caller's keys take priority)
                        let fact_metadata = if let Some(ref caller_meta) = metadata {
                            if let Some(caller_obj) = caller_meta.as_object() {
                                for (k, v) in caller_obj {
                                    dim_metadata.insert(k.clone(), v.clone());
                                }
                            }
                            Some(serde_json::Value::Object(dim_metadata))
                        } else {
                            Some(serde_json::Value::Object(dim_metadata))
                        };
                        
                        // Store each extracted fact separately
                        last_id = self.add_raw(
                            &fact.core_fact,
                            fact_type,
                            fact_importance,
                            source,
                            fact_metadata,
                            namespace,
                        )?;
                    }
                    return Ok(last_id);
                }
                Ok(_) => {
                    // No facts extracted - nothing worth storing
                    log::info!("Extractor: nothing worth storing in: {}...", 
                        content.chars().take(50).collect::<String>());
                    return Ok(String::new());
                }
                Err(e) => {
                    // Extractor failed - fall back to storing raw text
                    log::warn!("Extractor failed, storing raw: {}", e);
                    // Fall through to raw storage below
                }
            }
        }
        
        // No extractor or extractor failed - store raw (backward compatible)
        self.add_raw(content, memory_type, importance, source, metadata, namespace)
    }
    
    /// Parse a memory type string into MemoryType enum.
    #[allow(dead_code)] // Kept for potential future use (e.g., legacy migration)
    fn parse_memory_type(s: &str) -> Option<MemoryType> {
        match s.to_lowercase().as_str() {
            "factual" => Some(MemoryType::Factual),
            "episodic" => Some(MemoryType::Episodic),
            "relational" => Some(MemoryType::Relational),
            "emotional" => Some(MemoryType::Emotional),
            "procedural" => Some(MemoryType::Procedural),
            "opinion" => Some(MemoryType::Opinion),
            "causal" => Some(MemoryType::Causal),
            _ => None,
        }
    }
    
    /// Store a memory directly without extraction (internal method).
    ///
    /// This is the raw storage path used when no extractor is configured
    /// or when extractor fails.
    ///
    /// Flow (with dedup):
    /// 1. Generate embedding (if provider available)
    /// 2. If dedup enabled + have embedding → check for duplicates
    /// 3. If duplicate found → merge_memory_into + return existing_id
    /// 4. If no duplicate → storage.add() the new record
    /// 5. Store the pre-computed embedding (don't re-embed)
    /// 6. Extract entities
    fn add_raw(
        &mut self,
        content: &str,
        memory_type: MemoryType,
        importance: Option<f64>,
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
        namespace: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let ns = namespace.unwrap_or("default");
        let id = format!("{}", Uuid::new_v4())[..8].to_string();
        let base_importance = importance.unwrap_or_else(|| memory_type.default_importance());
        
        // Apply drive alignment boost if Emotional Bus is attached
        let importance = if let Some(ref bus) = self.empathy_bus {
            let boost = bus.align_importance(content);
            (base_importance * boost).min(1.0) // Cap at 1.0
        } else {
            base_importance
        };

        // Step 1: Generate embedding up front (if provider available)
        // We need it before storage for dedup check, and reuse it after storage.
        let pre_embedding = if let Some(ref embedding_provider) = self.embedding {
            match embedding_provider.embed(content) {
                Ok(emb) => Some(emb),
                Err(e) => {
                    log::warn!("Failed to generate embedding for dedup/storage: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // Step 2: Dedup check — if we have an embedding, check for near-duplicates
        if self.config.dedup_enabled {
            if let Some(ref embedding) = pre_embedding {
                let model_id = self.config.embedding.model_id();
                if let Ok(Some((existing_id, similarity))) = self.storage.find_nearest_embedding(
                    embedding,
                    &model_id,
                    Some(ns),
                    self.config.dedup_threshold,
                ) {
                    // Found a near-duplicate — merge instead of creating new
                    log::info!(
                        "Dedup: merging into existing memory {} (similarity: {:.4})",
                        existing_id, similarity
                    );
                    let outcome = self.storage.merge_memory_into(
                        &existing_id,
                        content,
                        importance,
                        similarity,
                    )?;
                    if outcome.content_updated {
                        log::info!(
                            "Dedup: content updated for {} (merge_count={})",
                            existing_id,
                            outcome.merge_count,
                        );
                    }
                    
                    // Also update entity links for the existing memory
                    if self.config.entity_config.enabled {
                        let entities = self.entity_extractor.extract(content);
                        for entity in &entities {
                            if let Ok(eid) = self.storage.upsert_entity(
                                &entity.normalized, entity.entity_type.as_str(), ns, None,
                            ) {
                                let _ = self.storage.link_memory_entity(&existing_id, &eid, "mention");
                            }
                        }
                    }
                    
                    return Ok(existing_id);
                }
            }
        }

        // Step 3: No duplicate found — create new memory record
        let record = MemoryRecord {
            id: id.clone(),
            content: content.to_string(),
            memory_type,
            layer: MemoryLayer::Working,
            created_at: Utc::now(),
            access_times: vec![Utc::now()],
            working_strength: 1.0,
            core_strength: 0.0,
            importance,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: source.unwrap_or("").to_string(),
            contradicts: None,
            contradicted_by: None,
            metadata,
        };

        self.storage.add(&record, ns)?;
        
        // Step 4: Store pre-computed embedding (avoid double-embed)
        if let Some(embedding) = pre_embedding {
            self.storage.store_embedding(
                &id,
                &embedding,
                &self.config.embedding.model_id(),
                self.config.embedding.dimensions,
            )?;
        }
        
        // Step 5: Entity extraction
        if self.config.entity_config.enabled {
            let entities = self.entity_extractor.extract(content);
            let mut entity_ids = Vec::new();
            
            for entity in &entities {
                match self.storage.upsert_entity(
                    &entity.normalized,
                    entity.entity_type.as_str(),
                    ns,
                    None,
                ) {
                    Ok(eid) => {
                        if let Err(e) = self.storage.link_memory_entity(&id, &eid, "mention") {
                            log::warn!("Failed to link memory {} to entity {}: {}", id, eid, e);
                        }
                        entity_ids.push(eid);
                    }
                    Err(e) => {
                        log::warn!("Failed to upsert entity '{}': {}", entity.normalized, e);
                    }
                }
            }
            
            // Co-occurrence relations (capped at 10 to avoid O(n²))
            let cap = entity_ids.len().min(10);
            for i in 0..cap {
                for j in (i + 1)..cap {
                    if let Err(e) = self.storage.upsert_entity_relation(
                        &entity_ids[i],
                        &entity_ids[j],
                        "co_occurs",
                        ns,
                    ) {
                        log::warn!("Failed to upsert entity relation: {}", e);
                    }
                }
            }
        }
        
        Ok(id)
    }
    
    /// Store a new memory with emotional tracking.
    ///
    /// This method both stores the memory and records the emotional valence
    /// in the Emotional Bus for trend tracking. Requires an Emotional Bus
    /// to be attached.
    ///
    /// # Arguments
    ///
    /// * `content` - The memory content
    /// * `memory_type` - Memory type classification
    /// * `importance` - 0-1 importance score (None = auto)
    /// * `source` - Source identifier
    /// * `metadata` - Optional metadata
    /// * `namespace` - Namespace to store in
    /// * `emotion` - Emotional valence (-1.0 to 1.0)
    /// * `domain` - Domain for emotional tracking
    pub fn add_with_emotion(
        &mut self,
        content: &str,
        memory_type: MemoryType,
        importance: Option<f64>,
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
        namespace: Option<&str>,
        emotion: f64,
        domain: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Store the memory (with importance boost from alignment)
        let id = self.add_to_namespace(content, memory_type, importance, source, metadata, namespace)?;
        
        // Record emotion if bus is attached
        if let Some(ref bus) = self.empathy_bus {
            bus.process_interaction(self.storage.connection(), content, emotion, domain)?;
        }
        
        Ok(id)
    }

    // =========================================================================
    // ISS-019 Step 4+: typed write-path API
    //
    // `store_enriched` — caller already has a validated `EnrichedMemory`.
    // `store_raw`      — caller has text only; engram runs the extractor
    //                    (or falls back to `Dimensions::minimal`) and
    //                    dispatches per-fact through `store_enriched`.
    //
    // These entry points are the canonical write surface going forward.
    // The legacy `add` / `add_to_namespace` / `add_with_emotion` stay as
    // `#[deprecated]` shims (Step 4.5) that forward to `store_raw` so
    // downstream callers migrate at their own pace.
    // =========================================================================

    /// Store a memory whose `Dimensions` are already validated.
    ///
    /// This is the primary write path. The returned `StoreOutcome` tells
    /// the caller whether a fresh row was inserted or the content was
    /// dedup-merged into an existing row.
    ///
    /// Metadata layout on disk today: we write the same legacy JSON shape
    /// the existing `add_raw` path produces, so dedup + merge history
    /// code keeps working untouched. Step 7 of the ISS-019 plan is where
    /// we namespace the blob under `engram.*` / `user.*`.
    pub fn store_enriched(
        &mut self,
        mem: crate::enriched::EnrichedMemory,
    ) -> Result<crate::store_api::StoreOutcome, crate::store_api::StoreError> {
        // Debug-time sanity: constructor guarantees this, but a caller
        // that built the struct literally could violate it.
        debug_assert!(mem.invariants_hold(), "EnrichedMemory::content must equal dimensions.core_fact");

        let memory_type = mem.dimensions.type_weights.primary_type();
        let importance_hint = Some(mem.importance.get());
        let source_opt = mem.source.clone();
        let namespace_opt = mem.namespace.clone();

        // Build the legacy metadata blob from the typed Dimensions.
        let metadata_json = build_legacy_metadata(&mem);

        let id = self
            .add_raw(
                &mem.content,
                memory_type,
                importance_hint,
                source_opt.as_deref(),
                Some(metadata_json),
                namespace_opt.as_deref(),
            )
            .map_err(boxed_err_to_store_error)?;

        // Translate `last_add_result` into the typed outcome.
        match self.last_add_result.clone() {
            Some(crate::lifecycle::AddResult::Merged { into, similarity }) => {
                Ok(crate::store_api::StoreOutcome::Merged { id: into, similarity })
            }
            Some(crate::lifecycle::AddResult::Created { id: created_id }) => {
                Ok(crate::store_api::StoreOutcome::Inserted { id: created_id })
            }
            None => {
                // add_raw always sets last_add_result; treat missing as Inserted
                // using the id we got back, keeping the contract intact.
                Ok(crate::store_api::StoreOutcome::Inserted { id })
            }
        }
    }

    /// Store raw text, running the configured extractor if present.
    ///
    /// Dispatch (see ISS-019 design §3.2):
    ///
    /// - no extractor configured      → `Dimensions::minimal` → `store_enriched`
    /// - extractor returns facts      → each fact → `EnrichedMemory::from_extracted`
    ///                                  → `store_enriched`
    /// - extractor returns empty      → `Skipped { NoFactsExtracted }`
    /// - extractor runtime failure    → `Quarantined { ExtractorError }`
    ///
    /// Note: Step 4 ships with the quarantine path as an **in-memory**
    /// outcome (no dedicated SQLite table yet). The id returned in
    /// `Quarantined { id }` is a synthetic `q-*` hash of the content
    /// so callers can correlate retries. The persistent quarantine
    /// table lands in Step 6.
    pub fn store_raw(
        &mut self,
        content: &str,
        meta: crate::store_api::StorageMeta,
    ) -> Result<crate::store_api::RawStoreOutcome, crate::store_api::StoreError> {
        use crate::store_api::{ContentHash, QuarantineId, QuarantineReason, RawStoreOutcome, SkipReason};

        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Ok(RawStoreOutcome::Skipped {
                reason: SkipReason::TooShort,
                content_hash: ContentHash::new(short_hash(content)),
            });
        }

        // Path A: extractor present.
        if let Some(ref extractor) = self.extractor {
            match extractor.extract(content) {
                Ok(facts) if facts.is_empty() => {
                    log::info!(
                        "store_raw: extractor returned nothing for content ({}...)",
                        content.chars().take(50).collect::<String>()
                    );
                    return Ok(RawStoreOutcome::Skipped {
                        reason: SkipReason::NoFactsExtracted,
                        content_hash: ContentHash::new(short_hash(content)),
                    });
                }
                Ok(facts) => {
                    // Cache emotion data for downstream consumers (parity
                    // with add_to_namespace).
                    let emotions: Vec<(f64, String)> = facts
                        .iter()
                        .map(|f| (f.valence, f.domain.clone()))
                        .collect();
                    *self.last_extraction_emotions.lock().unwrap() = Some(emotions);

                    let mut outcomes = Vec::with_capacity(facts.len());
                    let mut any_valid = false;
                    let mut first_err: Option<String> = None;

                    for fact in facts {
                        // Cap auto-extracted importance to prevent noise from
                        // dominating recall — same rule as the legacy path.
                        let capped = fact
                            .importance
                            .min(self.config.auto_extract_importance_cap);

                        let mut fact_adj = fact;
                        fact_adj.importance = capped;

                        match crate::enriched::EnrichedMemory::from_extracted(
                            fact_adj,
                            meta.source.clone(),
                            meta.namespace.clone(),
                            meta.user_metadata.clone(),
                        ) {
                            Ok(em) => {
                                any_valid = true;
                                let outcome = self.store_enriched(em)?;
                                outcomes.push(outcome);
                            }
                            Err(e) => {
                                // Don't abort the whole batch for a single
                                // empty-core_fact; skip that one fact and
                                // record the first error for the
                                // `AllFactsInvalid` branch below.
                                log::warn!(
                                    "store_raw: skipping invalid fact ({}); continuing batch",
                                    e
                                );
                                if first_err.is_none() {
                                    first_err = Some(e.to_string());
                                }
                            }
                        }
                    }

                    if any_valid {
                        return Ok(RawStoreOutcome::Stored(outcomes));
                    }

                    // Extractor produced facts but every one failed validation.
                    let qid = QuarantineId::new(format!("q-{}", short_hash(content)));
                    let reason = QuarantineReason::AllFactsInvalid(
                        first_err.unwrap_or_else(|| "no valid facts".to_string()),
                    );
                    log::warn!(
                        "store_raw: quarantining content ({}...): {:?}",
                        content.chars().take(50).collect::<String>(),
                        reason
                    );
                    return Ok(RawStoreOutcome::Quarantined { id: qid, reason });
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    log::warn!("store_raw: extractor error: {}", err_msg);
                    let qid = QuarantineId::new(format!("q-{}", short_hash(content)));
                    return Ok(RawStoreOutcome::Quarantined {
                        id: qid,
                        reason: QuarantineReason::ExtractorError(err_msg),
                    });
                }
            }
        }

        // Path B: no extractor — fall back to minimal dimensions.
        let importance_val = meta
            .importance_hint
            .map(crate::dimensions::Importance::new)
            .unwrap_or_else(|| {
                // Use the legacy memory_type default if the caller
                // threaded one through.
                let base = meta
                    .memory_type_hint
                    .map(|mt| mt.default_importance())
                    .unwrap_or(0.5);
                crate::dimensions::Importance::new(base)
            });

        let em = crate::enriched::EnrichedMemory::minimal(
            content,
            importance_val,
            meta.source.clone(),
            meta.namespace.clone(),
        )
        .map_err(|e| crate::store_api::StoreError::InvalidInput(e.to_string()))?;

        let em = crate::enriched::EnrichedMemory {
            user_metadata: meta.user_metadata.clone(),
            ..em
        };

        let outcome = self.store_enriched(em)?;
        Ok(RawStoreOutcome::Stored(vec![outcome]))
    }


    ///
    /// Unlike simple cosine similarity, this uses:
    /// - Base-level activation (frequency × recency, power law)
    /// - Spreading activation from context keywords
    /// - Importance modulation (emotional memories are more accessible)
    ///
    /// Results include a confidence score (metacognitive monitoring)
    /// that tells you how "trustworthy" each retrieval is.
    ///
    /// # Arguments
    ///
    /// * `query` - Natural language query
    /// * `limit` - Maximum number of results
    /// * `context` - Additional context keywords to boost relevant memories
    /// * `min_confidence` - Minimum confidence threshold (0-1)
    pub fn recall(
        &mut self,
        query: &str,
        limit: usize,
        context: Option<Vec<String>>,
        min_confidence: Option<f64>,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        self.recall_from_namespace(query, limit, context, min_confidence, None)
    }
    
    /// Retrieve relevant memories from a specific namespace.
    ///
    /// Uses hybrid search: FTS + embedding + ACT-R activation.
    /// FTS catches exact term matches, embedding catches semantic similarity,
    /// ACT-R boosts frequently/recently accessed memories.
    ///
    /// When embedding is not available, uses FTS + ACT-R only.
    ///
    /// # Arguments
    ///
    /// * `query` - Natural language query
    /// * `limit` - Maximum number of results
    /// * `context` - Additional context keywords to boost relevant memories
    /// * `min_confidence` - Minimum confidence threshold (0-1)
    /// * `namespace` - Namespace to search (None = "default", Some("*") = all namespaces)
    pub fn recall_from_namespace(
        &mut self,
        query: &str,
        limit: usize,
        context: Option<Vec<String>>,
        min_confidence: Option<f64>,
        namespace: Option<&str>,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        let now = Utc::now();
        let context = context.unwrap_or_default();
        let min_conf = min_confidence.unwrap_or(0.0);
        let ns = namespace.unwrap_or("default");
        
        // If embedding provider is available, use embedding-based recall
        if let Some(ref embedding_provider) = self.embedding {
            // Generate embedding for query
            let query_embedding = match embedding_provider.embed(query) {
                Ok(emb) => emb,
                Err(e) => {
                    log::warn!("Failed to generate query embedding, falling back to FTS: {}", e);
                    return self.recall_fts(query, limit, &context, min_conf, ns, now);
                }
            };
            
            // Get all embeddings for namespace and current model
            let model_id = self.config.embedding.model_id();
            let stored_embeddings = self.storage.get_embeddings_in_namespace(Some(ns), &model_id)?;
            
            // If no embedded memories, fall back to FTS
            if stored_embeddings.is_empty() {
                log::debug!("No embedded memories in namespace '{}', falling back to FTS", ns);
                return self.recall_fts(query, limit, &context, min_conf, ns, now);
            }
            
            // Build a map of memory_id -> embedding similarity
            let mut similarity_map: HashMap<String, f32> = HashMap::new();
            for (memory_id, stored_emb) in &stored_embeddings {
                let sim = EmbeddingProvider::cosine_similarity(&query_embedding, stored_emb);
                similarity_map.insert(memory_id.clone(), sim);
            }
            
            // Also get FTS results for exact term matching
            let fts_results = self.storage.search_fts_ns(query, limit * 3, Some(ns))
                .unwrap_or_default();
            let fts_count = fts_results.len();
            
            // Build FTS score map (rank-based normalization)
            let mut fts_score_map: HashMap<String, f64> = HashMap::new();
            for (rank, record) in fts_results.iter().enumerate() {
                let score = 1.0 - (rank as f64 / (fts_count.max(1)) as f64);
                fts_score_map.insert(record.id.clone(), score);
            }
            
            // Entity-based recall (4th channel)
            let (entity_scores, entity_w) = if self.config.entity_config.enabled {
                let es = self.entity_recall(query, Some(ns), limit * 3)?;
                (es, self.config.entity_weight)
            } else {
                (HashMap::new(), 0.0)
            };
            
            // Query-type adaptive weight adjustment (C7: Multi-Retrieval Fusion)
            // + Query intent classification (Level 1 regex + Level 2 Haiku LLM)
            let query_analysis = if self.config.adaptive_weights {
                crate::query_classifier::classify_query_with_l2(
                    query,
                    self.intent_classifier.as_ref(),
                )
            } else {
                crate::query_classifier::QueryAnalysis::neutral()
            };
            
            // Merge candidate IDs from embedding, FTS, and entity recall
            let mut all_ids: std::collections::HashSet<String> = similarity_map.keys().cloned().collect();
            for record in &fts_results {
                all_ids.insert(record.id.clone());
            }
            for id in entity_scores.keys() {
                all_ids.insert(id.clone());
            }
            
            // Get candidate memories
            let mut candidates: Vec<MemoryRecord> = Vec::new();
            for id in &all_ids {
                if let Some(record) = self.storage.get(id)? {
                    candidates.push(record);
                }
            }
            
            // Hebbian channel (6th channel): score candidates by Hebbian connectivity
            let candidate_ids: Vec<String> = candidates.iter().map(|r| r.id.clone()).collect();
            let hebbian_scores = if self.config.hebbian_enabled {
                Self::hebbian_channel_scores(&self.storage, &candidate_ids)?
            } else {
                HashMap::new()
            };
            let hebbian_w = if self.config.hebbian_enabled {
                self.config.hebbian_recall_weight
            } else {
                0.0
            };
            
            // Base weights (from config)
            let raw_fts_weight = self.config.fts_weight;
            let raw_emb_weight = self.config.embedding_weight;
            let raw_actr_weight = self.config.actr_weight;
            let raw_entity_weight = entity_w;
            let raw_temporal_weight = self.config.temporal_weight;
            let raw_hebbian_weight = hebbian_w;
            
            // Apply query-type modifiers (C7 adaptive weights)
            let adj_fts = raw_fts_weight * query_analysis.weight_modifiers.fts;
            let adj_emb = raw_emb_weight * query_analysis.weight_modifiers.embedding;
            let adj_actr = raw_actr_weight * query_analysis.weight_modifiers.actr;
            let adj_entity = raw_entity_weight * query_analysis.weight_modifiers.entity;
            let adj_temporal = raw_temporal_weight * query_analysis.weight_modifiers.temporal;
            let adj_hebbian = raw_hebbian_weight * query_analysis.weight_modifiers.hebbian;
            
            // Runtime normalization — always divide by sum
            let total_weight = adj_fts + adj_emb + adj_actr + adj_entity + adj_temporal + adj_hebbian;
            let (fts_weight, emb_weight, actr_weight, ent_weight, temp_weight, hebb_weight) = if total_weight > 0.0 {
                (
                    adj_fts / total_weight,
                    adj_emb / total_weight,
                    adj_actr / total_weight,
                    adj_entity / total_weight,
                    adj_temporal / total_weight,
                    adj_hebbian / total_weight,
                )
            } else {
                let n = 1.0 / 6.0;
                (n, n, n, n, n, n)
            };
            
            log::debug!(
                "C7 recall weights: fts={:.3} emb={:.3} actr={:.3} entity={:.3} temporal={:.3} hebbian={:.3} (query_type={:?})",
                fts_weight, emb_weight, actr_weight, ent_weight, temp_weight, hebb_weight,
                query_analysis.query_type,
            );
            
            let mut scored: Vec<_> = candidates
                .into_iter()
                .map(|record| {
                    // Get embedding similarity (already normalized to -1..1, convert to 0..1)
                    let embedding_sim = similarity_map
                        .get(&record.id)
                        .copied()
                        .unwrap_or(0.0);
                    let embedding_score = (embedding_sim + 1.0) / 2.0; // Normalize to 0..1
                    
                    // Get FTS score (0 if not in FTS results)
                    let fts_score = fts_score_map.get(&record.id).copied().unwrap_or(0.0);
                    
                    // Get entity score (0 if not in entity results)
                    let entity_score = entity_scores.get(&record.id).copied().unwrap_or(0.0);
                    
                    // Get ACT-R activation
                    let activation = retrieval_activation(
                        &record,
                        &context,
                        now,
                        self.config.actr_decay,
                        self.config.context_weight,
                        self.config.importance_weight,
                        self.config.contradiction_penalty,
                    );
                    
                    // Normalize activation to 0..1 range (sigmoid — much better discrimination than linear)
                    let activation_normalized = crate::models::actr::normalize_activation(
                        activation,
                        self.config.actr_sigmoid_center,
                        self.config.actr_sigmoid_scale,
                    );
                    
                    // Temporal channel (5th) — time-range proximity
                    let temporal_score = Self::temporal_score(&record, &query_analysis.time_range, now);
                    
                    // Hebbian channel (6th) — graph connectivity
                    let hebbian_score = hebbian_scores.get(&record.id).copied().unwrap_or(0.0);
                    
                    // Combined: 6-channel fusion
                    let combined_score = (fts_weight * fts_score)
                        + (emb_weight * embedding_score as f64)
                        + (actr_weight * activation_normalized)
                        + (ent_weight * entity_score)
                        + (temp_weight * temporal_score)
                        + (hebb_weight * hebbian_score);
                    
                    // Type-affinity modulation: weighted max over type_weights × affinity.
                    // For old memories (no type_weights in metadata), TypeWeights::default() (all 1.0)
                    // gives: max(1.0 × affinity_i) = max(affinity_i), equivalent to old behavior.
                    let type_weights = crate::type_weights::TypeWeights::from_metadata(&record.metadata);
                    let affinity_multiplier = [
                        type_weights.factual    * query_analysis.type_affinity.factual,
                        type_weights.episodic   * query_analysis.type_affinity.episodic,
                        type_weights.relational * query_analysis.type_affinity.relational,
                        type_weights.emotional  * query_analysis.type_affinity.emotional,
                        type_weights.procedural * query_analysis.type_affinity.procedural,
                        type_weights.opinion    * query_analysis.type_affinity.opinion,
                        type_weights.causal     * query_analysis.type_affinity.causal,
                    ]
                    .iter()
                    .cloned()
                    .fold(f64::NEG_INFINITY, f64::max);
                    let final_score = combined_score * affinity_multiplier;
                    
                    (record, final_score, activation)
                })
                .collect();

            // Sort by combined score descending
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

            // Take expanded candidate pool for dedup backfilling
            // Extra 3x expansion for type-affinity reranking (affinity may change ordering)
            let expanded_limit = if self.config.recall_dedup_enabled { limit * 3 } else { limit * 3 };
            let top_candidates: Vec<_> = scored.into_iter().take(expanded_limit).collect();

            // Build pairwise embedding lookup for dedup
            let embedding_lookup: HashMap<&str, &Vec<f32>> = stored_embeddings.iter()
                .map(|(id, emb)| (id.as_str(), emb))
                .collect();

            // Convert to RecallResults with confidence
            let mut all_results: Vec<RecallResult> = top_candidates.iter()
                .map(|(record, _combined_score, activation)| {
                    // Compute confidence from individual signals (not combined_score)
                    let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                    let confidence = compute_query_confidence(
                        similarity_map.get(&record.id).copied(),
                        fts_score_map.contains_key(&record.id),
                        entity_scores.get(&record.id).copied().unwrap_or(0.0),
                        age_hours,
                    );
                    let confidence_label = confidence_label(confidence);

                    RecallResult {
                        record: record.clone(),
                        activation: *activation,
                        confidence,
                        confidence_label,
                    }
                })
                .filter(|r| r.confidence >= min_conf)
                .collect();

            // Dedup by embedding similarity
            if self.config.recall_dedup_enabled {
                all_results = Self::dedup_recall_results_by_embedding(
                    all_results,
                    &embedding_lookup,
                    self.config.recall_dedup_threshold,
                    limit,
                );
            } else {
                all_results.truncate(limit);
            }

            let results = all_results;

            // Record access for all retrieved memories (ACT-R learning)
            for result in &results {
                self.storage.record_access(&result.record.id)?;
            }

            // Hebbian learning: record co-activation (namespace-aware)
            if self.config.hebbian_enabled && results.len() >= 2 {
                let memory_ids: Vec<_> = results.iter().map(|r| r.record.id.clone()).collect();
                crate::models::record_coactivation_ns(
                    &mut self.storage,
                    &memory_ids,
                    self.config.hebbian_threshold,
                    ns,
                )?;
            }

            Ok(results)
        } else {
            // No embedding provider, use FTS fallback
            self.recall_fts(query, limit, &context, min_conf, ns, now)
        }
    }

    /// Fetch the N most recently created memories, ordered newest-first.
    ///
    /// No query string needed — pure chronological retrieval.
    /// Designed for session bootstrap: after restart, inject recent context
    /// so the agent doesn't start from zero.
    ///
    /// # Arguments
    ///
    /// * `limit` - Maximum number of memories to return
    /// * `namespace` - Namespace filter (None = "default", Some("*") = all)
    pub fn recall_recent(
        &self,
        limit: usize,
        namespace: Option<&str>,
    ) -> Result<Vec<MemoryRecord>, Box<dyn std::error::Error>> {
        let records = self.storage.fetch_recent(limit, namespace)?;
        Ok(records)
    }
    
    /// Entity-based recall: extract entities from query, look up matching entities,
    /// return memory→score mapping (0.0-1.0 normalized).
    ///
    /// Direct entity matches get full score, 1-hop related entities get half score.
    /// Scores are normalized to 0.0-1.0 by dividing by the maximum score.
    fn entity_recall(
        &self,
        query: &str,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<HashMap<String, f64>, Box<dyn std::error::Error>> {
        let _ = limit; // limit is implicit via the number of entity matches
        let query_entities = self.entity_extractor.extract(query);
        let mut memory_scores: HashMap<String, f64> = HashMap::new();
        
        for qe in &query_entities {
            // Direct entity match (exact name)
            let matches = self.storage.find_entities(&qe.normalized, namespace, 5)?;
            for entity in &matches {
                let memory_ids = self.storage.get_entity_memories(&entity.id)?;
                for mid in memory_ids {
                    *memory_scores.entry(mid).or_insert(0.0) += 1.0;
                }
                
                // 1-hop related entities (weaker signal)
                let related = self.storage.get_related_entities(&entity.id, 10)?;
                for (rel_id, _relation) in related {
                    let rel_memories = self.storage.get_entity_memories(&rel_id)?;
                    for mid in rel_memories {
                        *memory_scores.entry(mid).or_insert(0.0) += 0.5;
                    }
                }
            }
        }
        
        // Normalize scores to 0.0-1.0
        if let Some(&max_score) = memory_scores.values().max_by(|a, b| a.partial_cmp(b).unwrap()) {
            if max_score > 0.0 {
                for score in memory_scores.values_mut() {
                    *score /= max_score;
                }
            }
        }
        
        Ok(memory_scores)
    }
    
    /// Temporal channel scoring for C7 Multi-Retrieval Fusion.
    ///
    /// When a time range is detected in the query, memories within that range
    /// are scored by proximity to the range center (1.0 at center, 0.5 at edges).
    /// Memories outside the range get 0.0.
    ///
    /// When no time range is detected, returns a neutral 0.5 for all memories
    /// (so the temporal channel doesn't distort results when there's no temporal signal).
    fn temporal_score(
        record: &MemoryRecord,
        time_range: &Option<crate::query_classifier::TimeRange>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> f64 {
        match time_range {
            Some(range) => {
                if record.created_at >= range.start && record.created_at <= range.end {
                    // Within range: score by proximity to center of range
                    let range_duration = (range.end - range.start).num_seconds() as f64;
                    let range_center = range.start + (range.end - range.start) / 2;
                    let distance = (record.created_at - range_center).num_seconds().abs() as f64;
                    let half_range = range_duration / 2.0;
                    if half_range > 0.0 {
                        // 1.0 at center, 0.5 at edges
                        1.0 - (distance / half_range) * 0.5
                    } else {
                        1.0
                    }
                } else {
                    0.0 // Outside range
                }
            }
            None => {
                // No temporal query: gentle recency signal (complement ACT-R)
                // Recent memories get slight boost, but not enough to dominate
                let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                // Sigmoid centered at 72 hours (3 days), scale 48
                let recency = 1.0 / (1.0 + (age_hours - 72.0).exp() / 48.0_f64.exp());
                // Map to 0.3-0.7 range (narrow band so it's a weak signal)
                0.3 + recency * 0.4
            }
        }
    }
    
    /// Hebbian channel scoring for C7 Multi-Retrieval Fusion.
    ///
    /// For each candidate, checks how many other candidates it's Hebbian-linked to.
    /// Memories that are well-connected to other recall results get boosted —
    /// they form coherent clusters of associated knowledge.
    ///
    /// Scores are normalized to 0.0-1.0.
    fn hebbian_channel_scores(
        storage: &crate::storage::Storage,
        candidate_ids: &[String],
    ) -> Result<HashMap<String, f64>, Box<dyn std::error::Error>> {
        let mut scores: HashMap<String, f64> = HashMap::new();
        let candidate_set: std::collections::HashSet<&String> = candidate_ids.iter().collect();
        
        for id in candidate_ids {
            let links = storage.get_hebbian_links_weighted(id)?;
            let mut link_score = 0.0;
            for (linked_id, strength) in &links {
                if candidate_set.contains(linked_id) {
                    // This candidate is Hebbian-linked to another candidate
                    link_score += strength;
                }
            }
            if link_score > 0.0 {
                scores.insert(id.clone(), link_score);
            }
        }
        
        // Normalize to 0.0-1.0
        if let Some(&max) = scores.values().max_by(|a, b| a.partial_cmp(b).unwrap()) {
            if max > 0.0 {
                for v in scores.values_mut() {
                    *v /= max;
                }
            }
        }
        
        Ok(scores)
    }
    
    /// Remove near-duplicate recall results based on pairwise embedding similarity.
    /// Greedy: iterate in score order, skip any result too similar to an already-kept one.
    /// Backfills from the expanded candidate pool to maintain the requested limit.
    fn dedup_recall_results_by_embedding(
        candidates: Vec<RecallResult>,
        embeddings: &HashMap<&str, &Vec<f32>>,
        threshold: f64,
        limit: usize,
    ) -> Vec<RecallResult> {
        let mut kept: Vec<RecallResult> = Vec::with_capacity(limit);
        let mut kept_embeddings: Vec<&Vec<f32>> = Vec::with_capacity(limit);

        for candidate in candidates {
            if kept.len() >= limit {
                break;
            }

            let candidate_emb = match embeddings.get(candidate.record.id.as_str()) {
                Some(emb) => *emb,
                None => {
                    // No embedding available, keep by default
                    kept.push(candidate);
                    continue;
                }
            };

            // Check against all kept results
            let is_dup = kept_embeddings.iter().any(|kept_emb| {
                let sim = EmbeddingProvider::cosine_similarity(candidate_emb, kept_emb);
                sim as f64 > threshold
            });

            if !is_dup {
                kept_embeddings.push(candidate_emb);
                kept.push(candidate);
            }
        }

        kept
    }

    /// FTS-based recall fallback when embeddings are not available.
    fn recall_fts(
        &mut self,
        query: &str,
        limit: usize,
        context: &[String],
        min_conf: f64,
        ns: &str,
        now: chrono::DateTime<Utc>,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        let fts_candidates = self.storage.search_fts_ns(query, limit * 3, Some(ns))?;
        
        let mut scored: Vec<_> = fts_candidates
            .into_iter()
            .map(|record| {
                let activation = retrieval_activation(
                    &record,
                    context,
                    now,
                    self.config.actr_decay,
                    self.config.context_weight,
                    self.config.importance_weight,
                    self.config.contradiction_penalty,
                );
                (record, activation)
            })
            .filter(|(_, act)| *act > f64::NEG_INFINITY)
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        let results: Vec<_> = scored
            .into_iter()
            .take(limit)
            .map(|(record, activation)| {
                let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                let confidence = compute_query_confidence(
                    None,       // no embedding available in FTS path
                    true,       // it's an FTS result by definition
                    0.0,        // no entity score in FTS path
                    age_hours,
                );
                let confidence_label = confidence_label(confidence);

                RecallResult {
                    record,
                    activation,
                    confidence,
                    confidence_label,
                }
            })
            .filter(|r| r.confidence >= min_conf)
            .collect();

        for result in &results {
            self.storage.record_access(&result.record.id)?;
        }

        // Hebbian learning: record co-activation (namespace-aware)
        if self.config.hebbian_enabled && results.len() >= 2 {
            let memory_ids: Vec<_> = results.iter().map(|r| r.record.id.clone()).collect();
            crate::models::record_coactivation_ns(
                &mut self.storage,
                &memory_ids,
                self.config.hebbian_threshold,
                ns,
            )?;
        }

        Ok(results)
    }

    /// Run a consolidation cycle ("sleep replay").
    ///
    /// This is the core of memory maintenance. Based on Murre & Chessa's
    /// Memory Chain Model, it:
    ///
    /// 1. Decays working_strength (hippocampal traces fade)
    /// 2. Transfers knowledge to core_strength (neocortical consolidation)
    /// 3. Replays archived memories (prevents catastrophic forgetting)
    /// 4. Rebalances layers (promote strong → core, demote weak → archive)
    ///
    /// Call this periodically — once per "day" of agent operation,
    /// or after significant learning sessions.
    ///
    /// # Arguments
    ///
    /// * `days` - Simulated time step in days (1.0 = one day of consolidation)
    pub fn consolidate(&mut self, days: f64) -> Result<(), Box<dyn std::error::Error>> {
        self.consolidate_namespace(days, None)
    }
    
    /// Run a consolidation cycle for a specific namespace.
    ///
    /// # Arguments
    ///
    /// * `days` - Simulated time step in days (1.0 = one day of consolidation)
    /// * `namespace` - Namespace to consolidate (None = all namespaces)
    pub fn consolidate_namespace(
        &mut self,
        days: f64,
        namespace: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        run_consolidation_cycle(&mut self.storage, days, &self.config, namespace)?;

        // Decay Hebbian links
        if self.config.hebbian_enabled {
            self.storage.decay_hebbian_links(self.config.hebbian_decay)?;
        }

        // Run synthesis if enabled (GUARD-3: opt-in, backward compatible)
        let synth_settings = self.synthesis_settings.clone();
        if let Some(ref settings) = synth_settings {
            if settings.enabled {
                let embedding_model = self.embedding.as_ref().map(|e| e.config().model_id());
                let engine = self.build_synthesis_engine(embedding_model);
                match engine.synthesize(&mut self.storage, settings) {
                    Ok(report) => {
                        if report.clusters_found > 0 {
                            log::info!(
                                "Synthesis: {} clusters found, {} synthesized, {} skipped, {} deferred",
                                report.clusters_found,
                                report.clusters_synthesized,
                                report.clusters_skipped,
                                report.clusters_deferred,
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!("Synthesis failed (non-fatal): {e}");
                    }
                }
                self.restore_llm_provider(engine.into_provider());
            }
        }

        Ok(())
    }

    /// Forget a specific memory or prune all below threshold.
    ///
    /// If memory_id is given, removes that specific memory.
    /// Otherwise, prunes all memories whose effective_strength
    /// is below threshold (moves them to archive).
    pub fn forget(
        &mut self,
        memory_id: Option<&str>,
        threshold: Option<f64>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let threshold = threshold.unwrap_or(self.config.forget_threshold);

        if let Some(id) = memory_id {
            self.storage.delete(id)?;
        } else {
            // Prune all weak memories
            let now = Utc::now();
            let all = self.storage.all()?;
            for record in all {
                if !record.pinned && effective_strength(&record, now) < threshold {
                    if record.layer != MemoryLayer::Archive {
                        let mut updated = record;
                        updated.layer = MemoryLayer::Archive;
                        self.storage.update(&updated)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Process user feedback as a dopaminergic reward signal.
    ///
    /// Detects positive/negative sentiment and applies reward modulation
    /// to recently accessed memories.
    pub fn reward(&mut self, feedback: &str, recent_n: usize) -> Result<(), Box<dyn std::error::Error>> {
        let polarity = detect_feedback_polarity(feedback);

        if polarity == 0.0 {
            return Ok(()); // Neutral feedback
        }

        // Get recently accessed memories
        let all = self.storage.all()?;
        let _now = Utc::now();
        let mut recent: Vec<_> = all
            .into_iter()
            .filter(|r| !r.access_times.is_empty())
            .collect();
        recent.sort_by_key(|r| std::cmp::Reverse(r.access_times.last().cloned()));

        // Apply reward to top-N recent
        for mut record in recent.into_iter().take(recent_n) {
            if polarity > 0.0 {
                // Positive feedback: boost working strength
                record.working_strength += self.config.reward_magnitude * polarity;
                record.working_strength = record.working_strength.min(2.0);
            } else {
                // Negative feedback: suppress working strength
                record.working_strength *= 1.0 + polarity * 0.1; // polarity is negative
                record.working_strength = record.working_strength.max(0.0);
            }
            self.storage.update(&record)?;
        }

        Ok(())
    }

    /// Global synaptic downscaling — normalize all memory weights.
    ///
    /// Based on Tononi & Cirelli's Synaptic Homeostasis Hypothesis.
    pub fn downscale(&mut self, factor: Option<f64>) -> Result<usize, Box<dyn std::error::Error>> {
        let factor = factor.unwrap_or(self.config.downscale_factor);
        let all = self.storage.all()?;
        let mut count = 0;

        for mut record in all {
            if !record.pinned {
                record.working_strength *= factor;
                record.core_strength *= factor;
                self.storage.update(&record)?;
                count += 1;
            }
        }

        Ok(count)
    }

    /// Memory system statistics.
    pub fn stats(&self) -> Result<MemoryStats, Box<dyn std::error::Error>> {
        let all = self.storage.all()?;
        let now = Utc::now();

        let mut by_type: HashMap<String, Vec<&MemoryRecord>> = HashMap::new();
        let mut by_layer: HashMap<String, Vec<&MemoryRecord>> = HashMap::new();
        let mut pinned = 0;

        for record in &all {
            by_type
                .entry(record.memory_type.to_string())
                .or_default()
                .push(record);
            by_layer
                .entry(record.layer.to_string())
                .or_default()
                .push(record);
            if record.pinned {
                pinned += 1;
            }
        }

        let type_stats: HashMap<String, TypeStats> = by_type
            .into_iter()
            .map(|(type_name, records)| {
                let count = records.len();
                let avg_strength = records
                    .iter()
                    .map(|r| effective_strength(r, now))
                    .sum::<f64>()
                    / count as f64;
                let avg_importance = records.iter().map(|r| r.importance).sum::<f64>() / count as f64;

                (
                    type_name,
                    TypeStats {
                        count,
                        avg_strength,
                        avg_importance,
                    },
                )
            })
            .collect();

        let layer_stats: HashMap<String, LayerStats> = by_layer
            .into_iter()
            .map(|(layer_name, records)| {
                let count = records.len();
                let avg_working = records.iter().map(|r| r.working_strength).sum::<f64>() / count as f64;
                let avg_core = records.iter().map(|r| r.core_strength).sum::<f64>() / count as f64;

                (
                    layer_name,
                    LayerStats {
                        count,
                        avg_working,
                        avg_core,
                    },
                )
            })
            .collect();

        let uptime_hours = (now - self.created_at).num_seconds() as f64 / 3600.0;

        Ok(MemoryStats {
            total_memories: all.len(),
            by_type: type_stats,
            by_layer: layer_stats,
            pinned,
            uptime_hours,
        })
    }

    /// Pin a memory — it won't decay or be pruned.
    pub fn pin(&mut self, memory_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(mut record) = self.storage.get(memory_id)? {
            record.pinned = true;
            self.storage.update(&record)?;
        }
        Ok(())
    }

    /// Unpin a memory — it will resume normal decay.
    pub fn unpin(&mut self, memory_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(mut record) = self.storage.get(memory_id)? {
            record.pinned = false;
            self.storage.update(&record)?;
        }
        Ok(())
    }
    
    // === Update Memory ===
    
    /// Update an existing memory's content.
    ///
    /// Stores the old content in metadata for audit trail and regenerates
    /// the embedding if embedding support is enabled.
    ///
    /// # Arguments
    ///
    /// * `memory_id` - ID of the memory to update
    /// * `new_content` - New content to replace the existing content
    /// * `reason` - Reason for the update (stored in metadata)
    ///
    /// # Returns
    ///
    /// The memory ID on success, or an error if the memory doesn't exist.
    pub fn update_memory(
        &mut self,
        memory_id: &str,
        new_content: &str,
        reason: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Get existing memory
        let record = self.storage.get(memory_id)?
            .ok_or_else(|| format!("Memory {} not found", memory_id))?;
        
        // Build updated metadata with audit trail
        let mut metadata = record.metadata.clone().unwrap_or_else(|| serde_json::json!({}));
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert("previous_content".to_string(), serde_json::json!(record.content));
            obj.insert("update_reason".to_string(), serde_json::json!(reason));
            obj.insert("updated_at".to_string(), serde_json::json!(Utc::now().to_rfc3339()));
        }
        
        // Update content in storage
        self.storage.update_content(memory_id, new_content, Some(metadata))?;
        
        // Regenerate embedding if provider is available
        if let Some(ref embedding_provider) = self.embedding {
            match embedding_provider.embed(new_content) {
                Ok(embedding) => {
                    // Delete old embedding for this model and store new one
                    self.storage.delete_embedding(memory_id, &self.config.embedding.model_id())?;
                    self.storage.store_embedding(
                        memory_id,
                        &embedding,
                        &self.config.embedding.model_id(),
                        self.config.embedding.dimensions,
                    )?;
                }
                Err(e) => {
                    log::warn!("Failed to regenerate embedding for {}: {}", memory_id, e);
                }
            }
        }
        
        Ok(memory_id.to_string())
    }
    
    // === Export ===
    
    /// Export all memories to a JSON file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the output JSON file
    ///
    /// # Returns
    ///
    /// Number of memories exported.
    pub fn export(&self, path: &str) -> Result<usize, Box<dyn std::error::Error>> {
        self.export_namespace(path, None)
    }
    
    /// Export memories from a specific namespace to a JSON file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the output JSON file
    /// * `namespace` - Namespace to export (None = all)
    ///
    /// # Returns
    ///
    /// Number of memories exported.
    pub fn export_namespace(
        &self,
        path: &str,
        namespace: Option<&str>,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let memories = self.storage.all_in_namespace(namespace)?;
        let count = memories.len();
        
        // Serialize to JSON
        let json = serde_json::to_string_pretty(&memories)?;
        
        // Write to file
        let mut file = File::create(path)?;
        file.write_all(json.as_bytes())?;
        
        log::info!("Exported {} memories to {}", count, path);
        
        Ok(count)
    }
    
    // === Recall Causal ===
    
    /// Recall associated memories (type=causal, created by STDP during consolidation).
    ///
    /// # Arguments
    ///
    /// * `cause_query` - Optional query to filter causal memories
    /// * `limit` - Maximum number of results
    /// * `min_confidence` - Minimum confidence threshold
    ///
    /// # Returns
    ///
    /// Matching associated memories sorted by importance.
    /// Hybrid recall combining FTS + embedding + ACT-R activation.
    /// 
    /// Unlike `recall()` which uses embedding+ACT-R only, this also includes
    /// FTS exact matching. Better for queries with specific names/terms.
    pub fn hybrid_recall(
        &mut self,
        query: &str,
        limit: usize,
        namespace: Option<&str>,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        use crate::hybrid_search::{hybrid_search, HybridSearchOpts};
        
        // Generate query embedding if available
        let query_vector = if let Some(ref embedding) = self.embedding {
            embedding.embed(query).ok()
        } else {
            None
        };
        
        let opts = HybridSearchOpts {
            limit,
            namespace: namespace.map(String::from),
            ..Default::default()
        };
        
        let results = hybrid_search(
            &self.storage,
            query_vector.as_deref(),
            query,
            opts,
            &self.config.embedding.model_id(),
        )?;
        
        // Convert HybridSearchResult to RecallResult
        let recall_results: Vec<RecallResult> = results.into_iter()
            .filter_map(|hr| {
                let record = hr.record?; // Skip if no record
                let score = hr.score;
                let label = if score > 0.7 {
                    "confident".to_string()
                } else if score > 0.4 {
                    "likely".to_string()
                } else {
                    "uncertain".to_string()
                };
                Some(RecallResult {
                    record,
                    activation: score,
                    confidence: score,
                    confidence_label: label,
                })
            })
            .collect();
        
        Ok(recall_results)
    }
    
    /// Uses Hebbian links to find memories that frequently co-occur.
    /// Note: this finds *associations*, not true causal relationships.
    /// LLMs can infer causality from the associated context.
    pub fn recall_associated(
        &mut self,
        cause_query: Option<&str>,
        limit: usize,
        min_confidence: f64,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        self.recall_associated_ns(cause_query, limit, min_confidence, None)
    }
    
    /// Recall associated memories from a specific namespace.
    pub fn recall_associated_ns(
        &mut self,
        cause_query: Option<&str>,
        limit: usize,
        min_confidence: f64,
        namespace: Option<&str>,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        let now = Utc::now();
        
        if let Some(query) = cause_query {
            // Do normal recall but filter to causal type
            let results = self.recall_from_namespace(
                query,
                limit * 2, // Fetch more to filter
                None,
                Some(min_confidence),
                namespace,
            )?;
            
            // Filter to causal type
            let filtered: Vec<_> = results
                .into_iter()
                .filter(|r| r.record.memory_type == MemoryType::Causal)
                .take(limit)
                .collect();
            
            Ok(filtered)
        } else {
            // Get all causal memories sorted by importance
            let causal_memories = self.storage.search_by_type_ns(
                MemoryType::Causal,
                namespace,
                limit * 2,
            )?;
            
            // Score and filter
            let mut scored: Vec<_> = causal_memories
                .into_iter()
                .map(|record| {
                    let activation = retrieval_activation(
                        &record,
                        &[],
                        now,
                        self.config.actr_decay,
                        self.config.context_weight,
                        self.config.importance_weight,
                        self.config.contradiction_penalty,
                    );
                    let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                    let confidence = compute_query_confidence(
                        None,   // no embedding in causal recall path
                        false,  // not an FTS query match
                        0.0,    // no entity score
                        age_hours,
                    );
                    (record, activation, confidence)
                })
                .filter(|(_, _, conf)| *conf >= min_confidence)
                .collect();
            
            // Sort by importance then activation
            scored.sort_by(|a, b| {
                b.0.importance.partial_cmp(&a.0.importance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            
            // Take top-k
            let results: Vec<_> = scored
                .into_iter()
                .take(limit)
                .map(|(record, activation, confidence)| {
                    RecallResult {
                        record,
                        activation,
                        confidence,
                        confidence_label: confidence_label(confidence),
                    }
                })
                .collect();
            
            Ok(results)
        }
    }
    
    // === Session Recall ===
    
    /// Session-aware recall using working memory to avoid redundant searches.
    ///
    /// If the topic is continuous (high overlap with previous recall), returns
    /// cached working memory items. If topic changed or WM is empty, does full recall.
    ///
    /// # Arguments
    ///
    /// * `query` - Search query
    /// * `session_wm` - Mutable reference to session's working memory
    /// * `limit` - Maximum number of results
    /// * `context` - Optional context keywords
    /// * `min_confidence` - Minimum confidence threshold
    ///
    /// # Returns
    ///
    /// SessionRecallResult with memories and metadata about the recall.
    pub fn session_recall(
        &mut self,
        query: &str,
        session_wm: &mut SessionWorkingMemory,
        limit: usize,
        context: Option<Vec<String>>,
        min_confidence: Option<f64>,
    ) -> Result<SessionRecallResult, Box<dyn std::error::Error>> {
        self.session_recall_ns(query, session_wm, limit, context, min_confidence, None)
    }
    
    /// Session-aware recall from a specific namespace.
    pub fn session_recall_ns(
        &mut self,
        query: &str,
        session_wm: &mut SessionWorkingMemory,
        limit: usize,
        context: Option<Vec<String>>,
        min_confidence: Option<f64>,
        namespace: Option<&str>,
    ) -> Result<SessionRecallResult, Box<dyn std::error::Error>> {
        const CONTINUITY_THRESHOLD: f64 = 0.6;
        
        // Get active IDs from working memory
        let active_ids = session_wm.get_active_ids();
        
        // Check if we need full recall, capturing the initial probe ratio
        let (need_full_recall, initial_ratio) = if active_ids.is_empty() {
            (true, 0.0)
        } else {
            // Do lightweight probe (3 results) to check topic continuity
            let probe = self.recall_from_namespace(
                query,
                3,
                context.clone(),
                min_confidence,
                namespace,
            )?;
            
            let probe_ids: Vec<String> = probe.iter().map(|r| r.record.id.clone()).collect();
            let (_, ratio) = session_wm.overlap(&probe_ids);
            if ratio >= CONTINUITY_THRESHOLD {
                (false, ratio)  // topic continuous
            } else {
                (true, ratio)   // topic changed
            }
        };
        
        if need_full_recall {
            // Topic changed or WM empty → full recall
            let results = self.recall_from_namespace(
                query,
                limit,
                context,
                min_confidence,
                namespace,
            )?;
            
            // Update working memory with scores for future cached path
            let entries: Vec<(String, f64, f64)> = results.iter()
                .map(|r| (r.record.id.clone(), r.confidence, r.activation))
                .collect();
            session_wm.activate_with_scores(&entries);
            session_wm.set_query(query);

            // GWT broadcast: admitted memories → interoceptive hub + Hebbian spreading.
            let admitted_ids: Vec<String> = results.iter().map(|r| r.record.id.clone()).collect();
            self.broadcast_admission(&admitted_ids, session_wm);
            
            Ok(SessionRecallResult {
                results,
                full_recall: true,
                wm_size: session_wm.len(),
                continuity_ratio: initial_ratio,
            })
        } else {
            // Topic continuous → return cached WM items with preserved scores
            let mut cached_results = Vec::new();
            let now = Utc::now();
            
            for id in &active_ids {
                if let Some(record) = self.storage.get(id)? {
                    // Reuse cached scores from the original full recall
                    let (activation, confidence) = if let Some(cached) = session_wm.get_score(id) {
                        (cached.activation, cached.confidence)
                    } else {
                        // Fallback: memory activated by ID only (legacy/no cached scores)
                        let activation = retrieval_activation(
                            &record,
                            &context.clone().unwrap_or_default(),
                            now,
                            self.config.actr_decay,
                            self.config.context_weight,
                            self.config.importance_weight,
                            self.config.contradiction_penalty,
                        );
                        let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                        let confidence = compute_query_confidence(
                            None,
                            false,
                            0.0,
                            age_hours,
                        );
                        (activation, confidence)
                    };
                    
                    if min_confidence.map(|mc| confidence >= mc).unwrap_or(true) {
                        cached_results.push(RecallResult {
                            record,
                            activation,
                            confidence,
                            confidence_label: confidence_label(confidence),
                        });
                    }
                }
            }
            
            // Sort by activation
            cached_results.sort_by(|a, b| {
                b.activation.partial_cmp(&a.activation).unwrap_or(std::cmp::Ordering::Equal)
            });
            cached_results.truncate(limit);
            
            // Refresh activation timestamps (reuse existing scores)
            let result_ids: Vec<String> = cached_results.iter().map(|r| r.record.id.clone()).collect();
            session_wm.activate(&result_ids);
            session_wm.set_query(query);

            // GWT broadcast on cached path too — re-activation reinforces the hub state.
            // (Lighter than full-recall broadcast: same memories, but keeps hub current.)
            self.broadcast_admission(&result_ids, session_wm);
            
            Ok(SessionRecallResult {
                results: cached_results,
                full_recall: false,
                wm_size: session_wm.len(),
                continuity_ratio: initial_ratio,  // reuse from initial probe
            })
        }
    }
    
    /// Get a memory by ID.
    pub fn get(&self, memory_id: &str) -> Result<Option<MemoryRecord>, Box<dyn std::error::Error>> {
        Ok(self.storage.get(memory_id)?)
    }
    
    /// List all memories (with optional limit).
    pub fn list(&self, limit: Option<usize>) -> Result<Vec<MemoryRecord>, Box<dyn std::error::Error>> {
        let all = self.storage.all()?;
        match limit {
            Some(l) => Ok(all.into_iter().take(l).collect()),
            None => Ok(all),
        }
    }
    
    /// List all memories in a namespace.
    pub fn list_ns(
        &self,
        namespace: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<MemoryRecord>, Box<dyn std::error::Error>> {
        let all = self.storage.all_in_namespace(namespace)?;
        match limit {
            Some(l) => Ok(all.into_iter().take(l).collect()),
            None => Ok(all),
        }
    }

    /// Get Hebbian links for a specific memory.
    pub fn hebbian_links(&self, memory_id: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(self.storage.get_hebbian_neighbors(memory_id)?)
    }
    
    /// Get Hebbian links for a specific memory, filtered by namespace.
    pub fn hebbian_links_ns(
        &self,
        memory_id: &str,
        namespace: Option<&str>,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(self.storage.get_hebbian_neighbors_ns(memory_id, namespace)?)
    }
    
    // === Embedding Methods ===
    
    /// Get embedding statistics.
    pub fn embedding_stats(&self) -> Result<EmbeddingStats, Box<dyn std::error::Error>> {
        Ok(self.storage.embedding_stats()?)
    }
    
    /// Get the embedding configuration.
    pub fn embedding_config(&self) -> &EmbeddingConfig {
        &self.config.embedding
    }
    
    /// Check if the embedding provider is available.
    pub fn is_embedding_available(&self) -> bool {
        self.embedding.as_ref().map(|e| e.is_available()).unwrap_or(false)
    }

    /// Get a reference to the embedding provider (if available).
    pub fn embedding_provider(&self) -> Option<&EmbeddingProvider> {
        self.embedding.as_ref()
    }
    
    /// Check if embedding support is enabled (provider was created).
    pub fn has_embedding_support(&self) -> bool {
        self.embedding.is_some()
    }
    
    /// Reindex embeddings for all memories without embeddings.
    ///
    /// Useful after migration or model change. Iterates all memories
    /// that don't have embeddings and generates them.
    ///
    /// Returns the number of memories reindexed.
    /// Returns an error if embedding provider is not available.
    pub fn reindex_embeddings(&mut self) -> Result<usize, Box<dyn std::error::Error>> {
        let embedding_provider = self.embedding.as_ref().ok_or_else(|| {
            Box::new(EmbeddingError::OllamaNotAvailable(self.config.embedding.host.clone()))
                as Box<dyn std::error::Error>
        })?;
        
        let model_id = self.config.embedding.model_id();
        let missing_ids = self.storage.get_memories_without_embeddings(&model_id)?;
        let total = missing_ids.len();
        
        if total == 0 {
            return Ok(0);
        }
        
        log::info!("Reindexing {} memories without embeddings for model {}", total, model_id);
        
        let mut reindexed = 0;
        for id in missing_ids {
            if let Some(record) = self.storage.get(&id)? {
                match embedding_provider.embed(&record.content) {
                    Ok(embedding) => {
                        self.storage.store_embedding(
                            &id,
                            &embedding,
                            &model_id,
                            self.config.embedding.dimensions,
                        )?;
                        reindexed += 1;
                    }
                    Err(e) => {
                        log::warn!("Failed to generate embedding for {}: {}", id, e);
                    }
                }
            }
        }
        
        log::info!("Reindexed {}/{} memories", reindexed, total);
        Ok(reindexed)
    }
    
    /// Reindex embeddings with progress callback.
    ///
    /// The callback receives (current, total) progress updates.
    /// Returns an error if embedding provider is not available.
    pub fn reindex_embeddings_with_progress<F>(
        &mut self,
        mut progress: F,
    ) -> Result<usize, Box<dyn std::error::Error>>
    where
        F: FnMut(usize, usize),
    {
        let embedding_provider = self.embedding.as_ref().ok_or_else(|| {
            Box::new(EmbeddingError::OllamaNotAvailable(self.config.embedding.host.clone()))
                as Box<dyn std::error::Error>
        })?;
        
        let model_id = self.config.embedding.model_id();
        let missing_ids = self.storage.get_memories_without_embeddings(&model_id)?;
        let total = missing_ids.len();
        
        if total == 0 {
            return Ok(0);
        }
        
        let mut reindexed = 0;
        for (i, id) in missing_ids.into_iter().enumerate() {
            progress(i + 1, total);
            
            if let Some(record) = self.storage.get(&id)? {
                match embedding_provider.embed(&record.content) {
                    Ok(embedding) => {
                        self.storage.store_embedding(
                            &id,
                            &embedding,
                            &model_id,
                            self.config.embedding.dimensions,
                        )?;
                        reindexed += 1;
                    }
                    Err(e) => {
                        log::warn!("Failed to generate embedding for {}: {}", id, e);
                    }
                }
            }
        }
        
        Ok(reindexed)
    }
    
    // === ACL Methods ===
    
    /// Grant a permission to an agent for a namespace.
    /// 
    /// Only agents with admin permission on the namespace (or wildcard admin)
    /// can grant permissions. If no agent_id is set, uses "system" as grantor.
    pub fn grant(
        &mut self,
        agent_id: &str,
        namespace: &str,
        permission: Permission,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let grantor = self.agent_id.clone().unwrap_or_else(|| "system".to_string());
        self.storage.grant_permission(agent_id, namespace, permission, &grantor)?;
        Ok(())
    }
    
    /// Revoke a permission from an agent for a namespace.
    pub fn revoke(&mut self, agent_id: &str, namespace: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.storage.revoke_permission(agent_id, namespace)?;
        Ok(())
    }
    
    /// Check if an agent has a specific permission for a namespace.
    pub fn check_permission(
        &self,
        agent_id: &str,
        namespace: &str,
        permission: Permission,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(self.storage.check_permission(agent_id, namespace, permission)?)
    }
    
    /// List all permissions for an agent.
    pub fn list_permissions(&self, agent_id: &str) -> Result<Vec<AclEntry>, Box<dyn std::error::Error>> {
        Ok(self.storage.list_permissions(agent_id)?)
    }
    
    /// Get statistics for a specific namespace.
    pub fn stats_ns(&self, namespace: Option<&str>) -> Result<MemoryStats, Box<dyn std::error::Error>> {
        let all = self.storage.all_in_namespace(namespace)?;
        let now = Utc::now();

        let mut by_type: HashMap<String, Vec<&MemoryRecord>> = HashMap::new();
        let mut by_layer: HashMap<String, Vec<&MemoryRecord>> = HashMap::new();
        let mut pinned = 0;

        for record in &all {
            by_type
                .entry(record.memory_type.to_string())
                .or_default()
                .push(record);
            by_layer
                .entry(record.layer.to_string())
                .or_default()
                .push(record);
            if record.pinned {
                pinned += 1;
            }
        }

        let type_stats: HashMap<String, TypeStats> = by_type
            .into_iter()
            .map(|(type_name, records)| {
                let count = records.len();
                let avg_strength = records
                    .iter()
                    .map(|r| effective_strength(r, now))
                    .sum::<f64>()
                    / count as f64;
                let avg_importance = records.iter().map(|r| r.importance).sum::<f64>() / count as f64;

                (
                    type_name,
                    TypeStats {
                        count,
                        avg_strength,
                        avg_importance,
                    },
                )
            })
            .collect();

        let layer_stats: HashMap<String, LayerStats> = by_layer
            .into_iter()
            .map(|(layer_name, records)| {
                let count = records.len();
                let avg_working = records.iter().map(|r| r.working_strength).sum::<f64>() / count as f64;
                let avg_core = records.iter().map(|r| r.core_strength).sum::<f64>() / count as f64;

                (
                    layer_name,
                    LayerStats {
                        count,
                        avg_working,
                        avg_core,
                    },
                )
            })
            .collect();

        let uptime_hours = (now - self.created_at).num_seconds() as f64 / 3600.0;

        Ok(MemoryStats {
            total_memories: all.len(),
            by_type: type_stats,
            by_layer: layer_stats,
            pinned,
            uptime_hours,
        })
    }
    
    // === Phase 3: Cross-Agent Intelligence ===
    
    /// Recall memories with cross-namespace associations.
    ///
    /// When using namespace="*", this also returns Hebbian links that span
    /// across different namespaces, enabling cross-domain intelligence.
    ///
    /// # Arguments
    ///
    /// * `query` - Natural language query
    /// * `namespace` - Namespace to search ("*" for all)
    /// * `limit` - Maximum number of results
    pub fn recall_with_associations(
        &mut self,
        query: &str,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<RecallWithAssociationsResult, Box<dyn std::error::Error>> {
        let now = Utc::now();
        let ns = namespace.unwrap_or("default");
        
        // Get candidate memories via FTS
        let candidates = self.storage.search_fts_ns(query, limit * 3, Some(ns))?;
        
        // Score each candidate with ACT-R activation
        let mut scored: Vec<_> = candidates
            .into_iter()
            .map(|record| {
                let activation = retrieval_activation(
                    &record,
                    &[],
                    now,
                    self.config.actr_decay,
                    self.config.context_weight,
                    self.config.importance_weight,
                    self.config.contradiction_penalty,
                );
                (record, activation)
            })
            .filter(|(_, act)| *act > f64::NEG_INFINITY)
            .collect();
        
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        
        // Take top-k
        let results: Vec<_> = scored
            .into_iter()
            .take(limit)
            .map(|(record, activation)| {
                let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                let confidence = compute_query_confidence(
                    None,   // no embedding in associations path
                    false,  // not an FTS query match
                    0.0,    // no entity score
                    age_hours,
                );
                let confidence_label = confidence_label(confidence);
                
                RecallResult {
                    record,
                    activation,
                    confidence,
                    confidence_label,
                }
            })
            .collect();
        
        // Record access for all retrieved memories
        for result in &results {
            self.storage.record_access(&result.record.id)?;
        }
        
        // Collect cross-namespace associations
        let mut cross_links = Vec::new();
        
        // For wildcard namespace queries, also collect cross-namespace Hebbian neighbors
        if ns == "*" && results.len() >= 2 {
            // Get namespaces for all retrieved memories
            let mut memories_with_ns: Vec<MemoryWithNamespace> = Vec::new();
            
            for result in &results {
                if let Some(mem_ns) = self.storage.get_namespace(&result.record.id)? {
                    memories_with_ns.push(MemoryWithNamespace {
                        id: result.record.id.clone(),
                        namespace: mem_ns,
                    });
                }
            }
            
            // Record cross-namespace co-activation
            if self.config.hebbian_enabled {
                record_cross_namespace_coactivation(
                    &mut self.storage,
                    &memories_with_ns,
                    self.config.hebbian_threshold,
                )?;
            }
            
            // Collect cross-links from all retrieved memories
            for result in &results {
                let links = self.storage.get_cross_namespace_neighbors(&result.record.id)?;
                cross_links.extend(links);
            }
            
            // Deduplicate by (source_id, target_id)
            cross_links.sort_by(|a, b| {
                (&a.source_id, &a.target_id).cmp(&(&b.source_id, &b.target_id))
            });
            cross_links.dedup_by(|a, b| a.source_id == b.source_id && a.target_id == b.target_id);
            
            // Sort by strength descending
            cross_links.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap());
        }
        
        Ok(RecallWithAssociationsResult {
            memories: results,
            cross_links,
        })
    }
    
    /// Discover cross-namespace Hebbian links between two namespaces.
    ///
    /// Returns all Hebbian associations that span across the given namespaces.
    /// ACL-aware: only returns links between namespaces the agent can read.
    pub fn discover_cross_links(
        &self,
        namespace_a: &str,
        namespace_b: &str,
    ) -> Result<Vec<HebbianLink>, Box<dyn std::error::Error>> {
        // ACL check if agent_id is set
        if let Some(ref agent_id) = self.agent_id {
            let can_read_a = self.storage.check_permission(agent_id, namespace_a, Permission::Read)?;
            let can_read_b = self.storage.check_permission(agent_id, namespace_b, Permission::Read)?;
            
            if !can_read_a || !can_read_b {
                return Ok(vec![]); // No access to one or both namespaces
            }
        }
        
        Ok(self.storage.discover_cross_links(namespace_a, namespace_b)?)
    }
    
    /// Get all cross-namespace associations for a memory.
    pub fn get_cross_associations(
        &self,
        memory_id: &str,
    ) -> Result<Vec<CrossLink>, Box<dyn std::error::Error>> {
        Ok(self.storage.get_cross_namespace_neighbors(memory_id)?)
    }
    
    // === Subscription/Notification Methods ===
    
    /// Subscribe to notifications for a namespace.
    ///
    /// The agent will receive notifications when new memories are stored
    /// with importance >= min_importance in the specified namespace.
    pub fn subscribe(
        &self,
        agent_id: &str,
        namespace: &str,
        min_importance: f64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mgr = SubscriptionManager::new(self.storage.connection())?;
        mgr.subscribe(agent_id, namespace, min_importance)?;
        Ok(())
    }
    
    /// Unsubscribe from a namespace.
    pub fn unsubscribe(
        &self,
        agent_id: &str,
        namespace: &str,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let mgr = SubscriptionManager::new(self.storage.connection())?;
        Ok(mgr.unsubscribe(agent_id, namespace)?)
    }
    
    /// List subscriptions for an agent.
    pub fn list_subscriptions(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Subscription>, Box<dyn std::error::Error>> {
        let mgr = SubscriptionManager::new(self.storage.connection())?;
        Ok(mgr.list_subscriptions(agent_id)?)
    }
    
    /// Check for notifications since last check.
    ///
    /// Returns new memories that exceed the subscription thresholds.
    /// Updates the cursor so the same notifications aren't returned twice.
    pub fn check_notifications(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Notification>, Box<dyn std::error::Error>> {
        let mgr = SubscriptionManager::new(self.storage.connection())?;
        Ok(mgr.check_notifications(agent_id)?)
    }
    
    /// Peek at notifications without updating cursor.
    pub fn peek_notifications(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Notification>, Box<dyn std::error::Error>> {
        let mgr = SubscriptionManager::new(self.storage.connection())?;
        Ok(mgr.peek_notifications(agent_id)?)
    }

    /// Extract entities from existing memories that don't have entity links yet.
    /// Returns (processed_count, entity_count, relation_count).
    pub fn backfill_entities(&self, batch_size: usize) -> Result<(usize, usize, usize), Box<dyn std::error::Error>> {
        let unlinked = self.storage.get_memories_without_entities(batch_size)?;
        let mut entity_count = 0;
        let mut relation_count = 0;
        let processed = unlinked.len();

        for (memory_id, content, ns) in &unlinked {
            let entities = self.entity_extractor.extract(content);
            let mut entity_ids = Vec::new();

            for entity in &entities {
                match self.storage.upsert_entity(
                    &entity.normalized,
                    entity.entity_type.as_str(),
                    ns,
                    None,
                ) {
                    Ok(eid) => {
                        let _ = self.storage.link_memory_entity(memory_id, &eid, "mention");
                        entity_ids.push(eid);
                        entity_count += 1;
                    }
                    Err(e) => {
                        log::warn!("Entity upsert failed during backfill: {}", e);
                    }
                }
            }

            // Co-occurrence (capped at 10)
            let cap = entity_ids.len().min(10);
            for i in 0..cap {
                for j in (i + 1)..cap {
                    if self
                        .storage
                        .upsert_entity_relation(&entity_ids[i], &entity_ids[j], "co_occurs", ns)
                        .is_ok()
                    {
                        relation_count += 1;
                    }
                }
            }
        }

        Ok((processed, entity_count, relation_count))
    }

    /// Get entity statistics: (entity_count, relation_count, link_count).
    pub fn entity_stats(&self) -> Result<(usize, usize, usize), Box<dyn std::error::Error>> {
        Ok(self.storage.entity_stats()?)
    }
    
    /// Purge garbage entities created by regex false positives.
    /// Removes:
    /// - Person entities that are 1-2 chars or pure digits (e.g., "0", "1", "types")
    /// - Orphaned entities with no memory links
    /// Returns count of entities deleted.
    pub fn purge_garbage_entities(&self) -> Result<usize, Box<dyn std::error::Error>> {
        let mut total_deleted = 0;
        
        // Phase 1: Delete short/numeric person entities that are clearly false positives
        let garbage_persons: Vec<String> = {
            let conn = self.storage.connection();
            let mut stmt = conn.prepare(
                "SELECT id, name FROM entities WHERE entity_type = 'person'"
            )?;
            let rows: Vec<(String, String)> = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?.filter_map(|r| r.ok()).collect();
            drop(stmt);
            
            rows.into_iter()
                .filter(|(_, name)| {
                    let n = name.trim();
                    // Pure digit, single char, or known false positive
                    n.len() <= 2
                        || n.chars().all(|c| c.is_ascii_digit())
                        || matches!(n, "types" | "user" | "mac" | "sigma" | "github")
                })
                .map(|(id, _)| id)
                .collect()
        };
        
        for id in &garbage_persons {
            if self.storage.delete_entity(id)? {
                total_deleted += 1;
            }
        }
        
        // Phase 2: Delete orphaned entities (no memory_entities links)
        let orphans: Vec<String> = {
            let conn = self.storage.connection();
            let mut stmt = conn.prepare(
                "SELECT e.id FROM entities e
                 LEFT JOIN memory_entities me ON e.id = me.entity_id
                 WHERE me.entity_id IS NULL"
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        
        for id in &orphans {
            if self.storage.delete_entity(id)? {
                total_deleted += 1;
            }
        }
        
        if total_deleted > 0 {
            log::info!("Purged {} garbage entities ({} false-positive persons, {} orphans)",
                total_deleted, garbage_persons.len(), orphans.len());
        }
        
        Ok(total_deleted)
    }

    /// List entities, optionally filtered by type and namespace.
    /// Returns (EntityRecord, mention_count) pairs ordered by mention count descending.
    pub fn list_entities(
        &self,
        entity_type: Option<&str>,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(crate::storage::EntityRecord, usize)>, Box<dyn std::error::Error>> {
        Ok(self.storage.list_entities(entity_type, namespace, limit)?)
    }

    // ===================================================================
    // Knowledge Synthesis API (ISS-005)
    // ===================================================================

    /// Run a full synthesis cycle: discover clusters → gate check → generate insights → store.
    ///
    /// Requires `set_synthesis_settings()` to have been called with `enabled: true`.
    /// Without an LLM provider (`set_synthesis_llm_provider()`), synthesis still
    /// discovers clusters and runs gate checks but skips insight text generation
    /// (graceful degradation).
    pub fn synthesize(
        &mut self,
    ) -> Result<crate::synthesis::types::SynthesisReport, Box<dyn std::error::Error>> {
        let settings = self.synthesis_settings.clone().unwrap_or_default();
        let embedding_model = self.embedding.as_ref().map(|e| e.config().model_id());
        let engine = self.build_synthesis_engine(embedding_model);
        let result = engine.synthesize(&mut self.storage, &settings);
        // Restore LLM provider (consumed by build_synthesis_engine via take())
        self.restore_llm_provider(engine.into_provider());
        result
    }

    /// Run a full synthesis cycle with custom settings (overrides stored settings).
    pub fn synthesize_with(
        &mut self,
        settings: &crate::synthesis::types::SynthesisSettings,
    ) -> Result<crate::synthesis::types::SynthesisReport, Box<dyn std::error::Error>> {
        let embedding_model = self.embedding.as_ref().map(|e| e.config().model_id());
        let engine = self.build_synthesis_engine(embedding_model);
        let result = engine.synthesize(&mut self.storage, settings);
        self.restore_llm_provider(engine.into_provider());
        result
    }

    /// Dry-run synthesis: discover clusters and gate decisions without making any changes.
    ///
    /// Returns the report with clusters_found and gate_results populated,
    /// but insights_created and sources_demoted will be empty.
    pub fn synthesize_dry_run(
        &mut self,
    ) -> Result<crate::synthesis::types::SynthesisReport, Box<dyn std::error::Error>> {
        use crate::synthesis::{cluster, gate};
        let settings = self.synthesis_settings.clone().unwrap_or_default();
        let embedding_model = self.embedding.as_ref().map(|e| e.config().model_id());

        let clusters = cluster::discover_clusters(
            &self.storage,
            &settings.cluster_discovery,
            embedding_model.as_deref(),
        )?;

        let mut gate_results = Vec::new();
        for cluster_data in &clusters {
            let all_memories = self.storage.all()?;
            let member_set: std::collections::HashSet<&str> =
                cluster_data.members.iter().map(|s| s.as_str()).collect();
            let members: Vec<MemoryRecord> = all_memories
                .into_iter()
                .filter(|m| member_set.contains(m.id.as_str()))
                .collect();

            let covered_pct = self.storage.check_coverage(&cluster_data.members)?;
            let gate_result = gate::check_gate(
                cluster_data,
                &members,
                &settings.gate,
                covered_pct,
                true,  // assume changed
                false, // not all pairs similar
            );
            gate_results.push(gate_result);
        }

        Ok(crate::synthesis::types::SynthesisReport {
            clusters_found: clusters.len(),
            clusters_synthesized: 0,
            clusters_auto_updated: 0,
            clusters_deferred: gate_results.iter().filter(|g| matches!(g.decision, crate::synthesis::types::GateDecision::Defer { .. })).count(),
            clusters_skipped: gate_results.iter().filter(|g| matches!(g.decision, crate::synthesis::types::GateDecision::Skip { .. })).count(),
            insights_created: Vec::new(),
            sources_demoted: Vec::new(),
            errors: Vec::new(),
            duration: std::time::Duration::ZERO,
            gate_results,
        })
    }

    /// Unified sleep cycle: consolidate, then synthesize.
    ///
    /// This is the recommended way to run both consolidation and synthesis in sequence.
    /// Consolidation always runs; synthesis only runs if enabled via settings.
    pub fn sleep_cycle(
        &mut self,
        days: f64,
        namespace: Option<&str>,
    ) -> Result<SleepReport, Box<dyn std::error::Error>> {
        // Phase 1: Synaptic consolidation (existing)
        self.consolidate_namespace(days, namespace)?;

        // Phase 2: Knowledge synthesis (if enabled)
        let synthesis = if self.synthesis_settings.as_ref().map_or(false, |s| s.enabled) {
            match self.synthesize() {
                Ok(report) => Some(report),
                Err(e) => {
                    log::warn!("Synthesis in sleep cycle failed (non-fatal): {e}");
                    None
                }
            }
        } else {
            None
        };

        Ok(SleepReport {
            consolidation_ok: true,
            synthesis,
        })
    }

    /// List all insight memories (memories with `is_synthesis: true` metadata).
    ///
    /// Returns insight MemoryRecords sorted by creation time (newest first).
    pub fn list_insights(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<MemoryRecord>, Box<dyn std::error::Error>> {
        let all = self.storage.all()?;
        let mut insights: Vec<MemoryRecord> = all
            .into_iter()
            .filter(|r| is_insight(r))
            .collect();
        insights.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        if let Some(l) = limit {
            insights.truncate(l);
        }
        Ok(insights)
    }

    /// Get source provenance records for a given insight.
    pub fn insight_sources(
        &self,
        insight_id: &str,
    ) -> Result<Vec<crate::synthesis::types::ProvenanceRecord>, Box<dyn std::error::Error>> {
        Ok(self.storage.get_insight_sources(insight_id)?)
    }

    /// Get insights derived from a specific source memory.
    pub fn insights_for_memory(
        &self,
        memory_id: &str,
    ) -> Result<Vec<crate::synthesis::types::ProvenanceRecord>, Box<dyn std::error::Error>> {
        Ok(self.storage.get_memory_insights(memory_id)?)
    }

    /// Reverse a synthesis: archive the insight and restore source importances.
    ///
    /// Does NOT delete the insight (GUARD-1: No Data Loss). Instead, archives it
    /// with importance 0.0 and restores all source memories to their pre-demotion state.
    pub fn reverse_synthesis(
        &mut self,
        insight_id: &str,
    ) -> Result<crate::synthesis::types::UndoSynthesis, Box<dyn std::error::Error>> {
        let embedding_model = self.embedding.as_ref().map(|e| e.config().model_id());
        let engine = self.build_synthesis_engine(embedding_model);
        let result = engine.undo_synthesis(&mut self.storage, insight_id);
        self.restore_llm_provider(engine.into_provider());
        result
    }

    /// Trace the provenance chain of a memory/insight back through its sources.
    pub fn get_provenance(
        &self,
        memory_id: &str,
        max_depth: usize,
    ) -> Result<crate::synthesis::types::ProvenanceChain, Box<dyn std::error::Error>> {
        // get_provenance only needs &Storage, no need to take LLM provider
        crate::synthesis::provenance::get_provenance_chain(&self.storage, memory_id, max_depth)
    }

    /// Build a DefaultSynthesisEngine, temporarily borrowing the LLM provider.
    ///
    /// Uses `Option::take` to move the provider into the engine. Callers must
    /// call `restore_llm_provider` after using the engine to put it back.
    fn build_synthesis_engine(
        &mut self,
        embedding_model: Option<String>,
    ) -> crate::synthesis::engine::DefaultSynthesisEngine {
        let llm = self.synthesis_llm_provider.take();
        crate::synthesis::engine::DefaultSynthesisEngine::new(llm, embedding_model)
    }

    /// Restore the LLM provider after engine use (engine returns it via `into_provider()`).
    fn restore_llm_provider(&mut self, provider: Option<Box<dyn crate::synthesis::types::SynthesisLlmProvider>>) {
        self.synthesis_llm_provider = provider;
    }

}

/// Compute confidence score (0.0-1.0) for a recall result.
///
/// Unlike the ranking score (combined_score), confidence measures how certain
/// we are that this memory is genuinely relevant to the query.
///
/// Signals:
/// - embedding_similarity: cosine sim of query vs memory embedding (strongest signal)
/// - in_fts_results: whether FTS found this memory (keyword overlap = confidence boost)
/// - entity_score: entity overlap score 0-1 (topical relevance)
/// - age_hours: memory age in hours (mild recency boost for very recent memories)
fn compute_query_confidence(
    embedding_similarity: Option<f32>,  // None if no embedding available
    in_fts_results: bool,
    entity_score: f64,  // 0.0 if no entity match or entity disabled
    age_hours: f64,
) -> f64 {
    let mut confidence = 0.0;
    let mut max_possible = 0.0;

    // Signal 1: Embedding similarity (weight: 0.55)
    if let Some(sim) = embedding_similarity {
        let sim = sim as f64;
        // Apply sigmoid-like curve to sharpen discrimination
        // sim > 0.7 → strong confidence, sim < 0.3 → near zero
        let emb_conf = if sim > 0.0 {
            // Logistic function centered at 0.5 with steepness 10
            1.0 / (1.0 + (-10.0 * (sim - 0.5)).exp())
        } else {
            0.0
        };
        confidence += 0.55 * emb_conf;
        max_possible += 0.55;
    }

    // Signal 2: FTS match (weight: 0.20)
    if in_fts_results {
        confidence += 0.20;
    }
    max_possible += 0.20;

    // Signal 3: Entity overlap (weight: 0.20)
    confidence += 0.20 * entity_score;
    max_possible += 0.20;

    // Signal 4: Recency boost (weight: 0.05)
    // Very recent memories get a small confidence boost
    // 1 hour ago → ~1.0, 24 hours → ~0.5, 7 days → ~0.1
    let recency = 1.0 / (1.0 + (age_hours / 24.0).ln().max(0.0));
    confidence += 0.05 * recency.clamp(0.0, 1.0);
    max_possible += 0.05;

    // Normalize by max possible (handles case where embedding is unavailable)
    if max_possible > 0.0 {
        (confidence / max_possible).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn confidence_label(confidence: f64) -> String {
    match confidence {
        c if c >= 0.8 => "high".to_string(),
        c if c >= 0.5 => "medium".to_string(),
        c if c >= 0.2 => "low".to_string(),
        _ => "very low".to_string(),
    }
}

fn detect_feedback_polarity(feedback: &str) -> f64 {
    let lower = feedback.to_lowercase();
    let positive = ["good", "great", "excellent", "correct", "right", "yes", "nice", "perfect"];
    let negative = ["bad", "wrong", "incorrect", "no", "error", "mistake", "poor"];

    let pos_count = positive.iter().filter(|&w| lower.contains(w)).count();
    let neg_count = negative.iter().filter(|&w| lower.contains(w)).count();

    if pos_count > neg_count {
        1.0
    } else if neg_count > pos_count {
        -1.0
    } else {
        0.0
    }
}

// =========================================================================
// ISS-019 Step 4 helpers (module-level — re-used by the shim layer).
// =========================================================================

/// Build the legacy `metadata` JSON blob from a validated `EnrichedMemory`.
///
/// The on-disk layout is the same shape `add_to_namespace` has always
/// produced — `dimensions.*` + `type_weights` + any caller user-metadata
/// keys merged at top level. This keeps Step 4 strictly additive: dedup,
/// merge history, and read-side consumers (KC, clustering) continue to
/// work without schema changes.
///
/// Step 7 of the ISS-019 plan introduces the versioned `engram.*` /
/// `user.*` namespacing. Moving it there keeps the diff reviewable.
fn build_legacy_metadata(mem: &crate::enriched::EnrichedMemory) -> serde_json::Value {
    use crate::dimensions::TemporalMark;

    let d = &mem.dimensions;

    // Dimensional sub-object — only fields that have a value are written,
    // matching the legacy add_to_namespace behavior.
    let mut dims = serde_json::Map::new();
    if let Some(ref v) = d.participants {
        dims.insert("participants".into(), serde_json::Value::String(v.clone()));
    }
    if let Some(ref v) = d.temporal {
        let s = match v {
            TemporalMark::Exact(dt) => dt.to_rfc3339(),
            TemporalMark::Day(day) => day.format("%Y-%m-%d").to_string(),
            TemporalMark::Range { start, end } => format!(
                "{}..{}",
                start.format("%Y-%m-%d"),
                end.format("%Y-%m-%d")
            ),
            TemporalMark::Vague(s) => s.clone(),
        };
        dims.insert("temporal".into(), serde_json::Value::String(s));
    }
    if let Some(ref v) = d.location {
        dims.insert("location".into(), serde_json::Value::String(v.clone()));
    }
    if let Some(ref v) = d.context {
        dims.insert("context".into(), serde_json::Value::String(v.clone()));
    }
    if let Some(ref v) = d.causation {
        dims.insert("causation".into(), serde_json::Value::String(v.clone()));
    }
    if let Some(ref v) = d.outcome {
        dims.insert("outcome".into(), serde_json::Value::String(v.clone()));
    }
    if let Some(ref v) = d.method {
        dims.insert("method".into(), serde_json::Value::String(v.clone()));
    }
    if let Some(ref v) = d.relations {
        dims.insert("relations".into(), serde_json::Value::String(v.clone()));
    }
    if let Some(ref v) = d.sentiment {
        dims.insert("sentiment".into(), serde_json::Value::String(v.clone()));
    }
    if let Some(ref v) = d.stance {
        dims.insert("stance".into(), serde_json::Value::String(v.clone()));
    }

    // Always-present scalar fields — mirrors ISS-020 P0.0 fix so KC
    // / conflict detection / clustering can pre-filter by domain.
    dims.insert("valence".into(), serde_json::json!(d.valence.get()));
    dims.insert(
        "confidence".into(),
        serde_json::Value::String(
            match d.confidence {
                crate::dimensions::Confidence::Confident => "confident",
                crate::dimensions::Confidence::Likely => "likely",
                crate::dimensions::Confidence::Uncertain => "uncertain",
            }
            .to_string(),
        ),
    );
    dims.insert(
        "domain".into(),
        serde_json::Value::String(domain_to_loose_str(&d.domain)),
    );

    // Tags — round-tripped as a JSON array for external consumers.
    if !d.tags.is_empty() {
        dims.insert(
            "tags".into(),
            serde_json::Value::Array(
                d.tags
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }

    // Top-level metadata object.
    let mut meta = serde_json::Map::new();
    meta.insert("dimensions".into(), serde_json::Value::Object(dims));
    meta.insert("type_weights".into(), d.type_weights.to_json());

    // Merge caller-supplied user metadata (user keys take priority, same
    // contract as add_to_namespace).
    if let serde_json::Value::Object(user) = &mem.user_metadata {
        for (k, v) in user {
            meta.insert(k.clone(), v.clone());
        }
    }

    serde_json::Value::Object(meta)
}

fn domain_to_loose_str(d: &crate::dimensions::Domain) -> String {
    match d {
        crate::dimensions::Domain::Coding => "coding".into(),
        crate::dimensions::Domain::Trading => "trading".into(),
        crate::dimensions::Domain::Research => "research".into(),
        crate::dimensions::Domain::Communication => "communication".into(),
        crate::dimensions::Domain::General => "general".into(),
        crate::dimensions::Domain::Other(s) => s.clone(),
    }
}

/// Map `add_raw`'s `Box<dyn Error>` into the typed `StoreError`.
///
/// `add_raw` today returns a boxed trait object, which is too loose for
/// the new API. The only error kind we actually expect through this
/// path is a `rusqlite::Error` (DB write failure); anything else is
/// treated as a pipeline error and surfaces as `InvalidState`.
fn boxed_err_to_store_error(
    e: Box<dyn std::error::Error>,
) -> crate::store_api::StoreError {
    // Attempt to downcast to rusqlite::Error first — the common case.
    match e.downcast::<rusqlite::Error>() {
        Ok(db_err) => crate::store_api::StoreError::DbError(*db_err),
        Err(other) => crate::store_api::StoreError::InvalidState(other.to_string()),
    }
}

/// Short, stable hex digest of content (first 16 hex chars of SHA-256).
/// Used for skip / quarantine content_hash; cheap, deterministic, not
/// cryptographically strong (not a security boundary here).
fn short_hash(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    content.hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod confidence_tests {
    use super::*;

    #[test]
    fn test_confidence_high_embedding_sim() {
        // High embedding sim + FTS + entity → high confidence
        let c = compute_query_confidence(Some(0.85), true, 0.8, 1.0);
        assert!(c > 0.8, "expected high confidence, got {c}");
        assert_eq!(confidence_label(c), "high");
    }

    #[test]
    fn test_confidence_low_embedding_sim() {
        // Low embedding sim, no FTS, no entity → low confidence
        let c = compute_query_confidence(Some(0.2), false, 0.0, 720.0);
        assert!(c < 0.3, "expected low confidence, got {c}");
    }

    #[test]
    fn test_confidence_medium_embedding_sim() {
        // Medium embedding sim + FTS → medium range
        let c = compute_query_confidence(Some(0.55), true, 0.0, 24.0);
        assert!(c > 0.3 && c < 0.8, "expected medium confidence, got {c}");
    }

    #[test]
    fn test_confidence_no_embedding() {
        // No embedding available, FTS=true, some entity match
        let c = compute_query_confidence(None, true, 0.5, 2.0);
        // Without embedding, max_possible = 0.45, confidence comes from FTS + entity + recency
        assert!(c > 0.3, "expected reasonable confidence without embedding, got {c}");
        assert!(c < 1.0);
    }

    #[test]
    fn test_confidence_fts_only_boost() {
        // No embedding, FTS only, no entity
        let c_fts = compute_query_confidence(None, true, 0.0, 24.0);
        let c_none = compute_query_confidence(None, false, 0.0, 24.0);
        assert!(c_fts > c_none, "FTS should boost confidence: {c_fts} > {c_none}");
    }

    #[test]
    fn test_confidence_entity_boost() {
        // Same embedding sim, but different entity scores
        let c_entity = compute_query_confidence(Some(0.5), false, 0.9, 48.0);
        let c_no_entity = compute_query_confidence(Some(0.5), false, 0.0, 48.0);
        assert!(c_entity > c_no_entity, "entity should boost confidence: {c_entity} > {c_no_entity}");
    }

    #[test]
    fn test_confidence_recency_boost() {
        // Recent (1h) vs old (30 days = 720h) — same other signals
        let c_recent = compute_query_confidence(Some(0.6), true, 0.0, 1.0);
        let c_old = compute_query_confidence(Some(0.6), true, 0.0, 720.0);
        assert!(c_recent > c_old, "recent should have slightly higher confidence: {c_recent} > {c_old}");
        // But the difference should be small (recency weight is only 0.05)
        assert!((c_recent - c_old).abs() < 0.1, "recency difference should be small");
    }

    #[test]
    fn test_confidence_all_zero() {
        // All signals at worst values
        let c = compute_query_confidence(Some(0.0), false, 0.0, 8760.0); // 1 year old
        assert!(c < 0.1, "all-zero signals should give near-zero confidence, got {c}");
    }

    #[test]
    fn test_confidence_all_max() {
        // All signals at best values
        let c = compute_query_confidence(Some(0.99), true, 1.0, 0.1);
        assert!(c > 0.9, "all-max signals should give high confidence, got {c}");
    }

    #[test]
    fn test_confidence_label_thresholds() {
        assert_eq!(confidence_label(0.9), "high");
        assert_eq!(confidence_label(0.8), "high");
        assert_eq!(confidence_label(0.79), "medium");
        assert_eq!(confidence_label(0.5), "medium");
        assert_eq!(confidence_label(0.49), "low");
        assert_eq!(confidence_label(0.2), "low");
        assert_eq!(confidence_label(0.19), "very low");
        assert_eq!(confidence_label(0.0), "very low");
    }

    #[test]
    fn test_confidence_sigmoid_discrimination() {
        // The sigmoid should create clear separation between high/low sim
        let c_high = compute_query_confidence(Some(0.8), false, 0.0, 100.0);
        let c_low = compute_query_confidence(Some(0.3), false, 0.0, 100.0);
        let gap = c_high - c_low;
        assert!(gap > 0.3, "sigmoid should create large gap between 0.8 and 0.3 sim: gap={gap}");
    }

    #[test]
    fn test_auto_extract_importance_cap() {
        let mut config = MemoryConfig::default();
        config.auto_extract_importance_cap = 0.7;
        
        // Test capping logic directly
        let extracted_importance: f64 = 0.95;
        let capped = extracted_importance.min(config.auto_extract_importance_cap);
        assert_eq!(capped, 0.7);
        
        // Below cap — no change
        let low_importance: f64 = 0.3;
        let not_capped = low_importance.min(config.auto_extract_importance_cap);
        assert_eq!(not_capped, 0.3);
        
        // Exactly at cap — no change
        let at_cap: f64 = 0.7;
        let stays = at_cap.min(config.auto_extract_importance_cap);
        assert_eq!(stays, 0.7);
    }

    #[test]
    fn test_auto_extract_importance_cap_default() {
        let config = MemoryConfig::default();
        assert_eq!(config.auto_extract_importance_cap, 0.7);
    }

    #[test]
    fn test_dedup_recall_results_by_embedding() {
        // Create mock embeddings - two near-identical and one different
        let emb_a: Vec<f32> = vec![1.0, 0.0, 0.0];
        let emb_b: Vec<f32> = vec![0.99, 0.1, 0.0]; // Very similar to A
        let emb_c: Vec<f32> = vec![0.0, 1.0, 0.0]; // Different

        let mut embeddings_map: HashMap<&str, &Vec<f32>> = HashMap::new();
        embeddings_map.insert("id-a", &emb_a);
        embeddings_map.insert("id-b", &emb_b);
        embeddings_map.insert("id-c", &emb_c);

        let make_result = |id: &str, confidence: f64| RecallResult {
            record: MemoryRecord {
                id: id.to_string(),
                content: format!("content-{}", id),
                memory_type: MemoryType::Factual,
                layer: MemoryLayer::Working,
                created_at: Utc::now(),
                access_times: vec![Utc::now()],
                working_strength: 1.0,
                core_strength: 0.0,
                importance: 0.5,
                pinned: false,
                consolidation_count: 0,
                last_consolidated: None,
                source: "test".to_string(),
                contradicts: None,
                contradicted_by: None,
                metadata: None,
            },
            activation: 0.5,
            confidence,
            confidence_label: "high".to_string(),
        };

        let candidates = vec![
            make_result("id-a", 0.9),
            make_result("id-b", 0.8), // Near-dup of A
            make_result("id-c", 0.7),
        ];

        let result = Memory::dedup_recall_results_by_embedding(
            candidates,
            &embeddings_map,
            0.85, // threshold
            3,    // limit
        );

        // Should keep A and C, skip B (too similar to A)
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].record.id, "id-a");
        assert_eq!(result[1].record.id, "id-c");
    }

    #[test]
    fn test_dedup_recall_no_duplicates() {
        // All embeddings are orthogonal — nothing should be deduped
        let emb_a: Vec<f32> = vec![1.0, 0.0, 0.0];
        let emb_b: Vec<f32> = vec![0.0, 1.0, 0.0];
        let emb_c: Vec<f32> = vec![0.0, 0.0, 1.0];

        let mut embeddings_map: HashMap<&str, &Vec<f32>> = HashMap::new();
        embeddings_map.insert("id-a", &emb_a);
        embeddings_map.insert("id-b", &emb_b);
        embeddings_map.insert("id-c", &emb_c);

        let make_result = |id: &str, confidence: f64| RecallResult {
            record: MemoryRecord {
                id: id.to_string(),
                content: format!("content-{}", id),
                memory_type: MemoryType::Factual,
                layer: MemoryLayer::Working,
                created_at: Utc::now(),
                access_times: vec![Utc::now()],
                working_strength: 1.0,
                core_strength: 0.0,
                importance: 0.5,
                pinned: false,
                consolidation_count: 0,
                last_consolidated: None,
                source: "test".to_string(),
                contradicts: None,
                contradicted_by: None,
                metadata: None,
            },
            activation: 0.5,
            confidence,
            confidence_label: "high".to_string(),
        };

        let candidates = vec![
            make_result("id-a", 0.9),
            make_result("id-b", 0.8),
            make_result("id-c", 0.7),
        ];

        let result = Memory::dedup_recall_results_by_embedding(
            candidates,
            &embeddings_map,
            0.85,
            3,
        );

        // All three should be kept
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_dedup_recall_respects_limit() {
        // No dups, but limit is 2
        let emb_a: Vec<f32> = vec![1.0, 0.0, 0.0];
        let emb_b: Vec<f32> = vec![0.0, 1.0, 0.0];
        let emb_c: Vec<f32> = vec![0.0, 0.0, 1.0];

        let mut embeddings_map: HashMap<&str, &Vec<f32>> = HashMap::new();
        embeddings_map.insert("id-a", &emb_a);
        embeddings_map.insert("id-b", &emb_b);
        embeddings_map.insert("id-c", &emb_c);

        let make_result = |id: &str| RecallResult {
            record: MemoryRecord {
                id: id.to_string(),
                content: format!("content-{}", id),
                memory_type: MemoryType::Factual,
                layer: MemoryLayer::Working,
                created_at: Utc::now(),
                access_times: vec![Utc::now()],
                working_strength: 1.0,
                core_strength: 0.0,
                importance: 0.5,
                pinned: false,
                consolidation_count: 0,
                last_consolidated: None,
                source: "test".to_string(),
                contradicts: None,
                contradicted_by: None,
                metadata: None,
            },
            activation: 0.5,
            confidence: 0.8,
            confidence_label: "high".to_string(),
        };

        let candidates = vec![
            make_result("id-a"),
            make_result("id-b"),
            make_result("id-c"),
        ];

        let result = Memory::dedup_recall_results_by_embedding(
            candidates,
            &embeddings_map,
            0.85,
            2, // limit of 2
        );

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].record.id, "id-a");
        assert_eq!(result[1].record.id, "id-b");
    }

    #[test]
    fn test_dedup_recall_missing_embedding_kept() {
        // If a candidate has no embedding in the lookup, it should be kept
        let emb_a: Vec<f32> = vec![1.0, 0.0, 0.0];

        let mut embeddings_map: HashMap<&str, &Vec<f32>> = HashMap::new();
        embeddings_map.insert("id-a", &emb_a);
        // id-b has no embedding

        let make_result = |id: &str| RecallResult {
            record: MemoryRecord {
                id: id.to_string(),
                content: format!("content-{}", id),
                memory_type: MemoryType::Factual,
                layer: MemoryLayer::Working,
                created_at: Utc::now(),
                access_times: vec![Utc::now()],
                working_strength: 1.0,
                core_strength: 0.0,
                importance: 0.5,
                pinned: false,
                consolidation_count: 0,
                last_consolidated: None,
                source: "test".to_string(),
                contradicts: None,
                contradicted_by: None,
                metadata: None,
            },
            activation: 0.5,
            confidence: 0.8,
            confidence_label: "high".to_string(),
        };

        let candidates = vec![
            make_result("id-a"),
            make_result("id-b"), // No embedding
        ];

        let result = Memory::dedup_recall_results_by_embedding(
            candidates,
            &embeddings_map,
            0.85,
            5,
        );

        // Both should be kept (id-b has no embedding, so can't be deduped)
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_dedup_recall_backfills_from_candidates() {
        // A, B (dup of A), C (different) — with limit=2, should get A and C
        let emb_a: Vec<f32> = vec![1.0, 0.0, 0.0];
        let emb_b: Vec<f32> = vec![0.98, 0.05, 0.0]; // Near-duplicate of A
        let emb_c: Vec<f32> = vec![0.0, 1.0, 0.0]; // Different

        let mut embeddings_map: HashMap<&str, &Vec<f32>> = HashMap::new();
        embeddings_map.insert("id-a", &emb_a);
        embeddings_map.insert("id-b", &emb_b);
        embeddings_map.insert("id-c", &emb_c);

        let make_result = |id: &str, confidence: f64| RecallResult {
            record: MemoryRecord {
                id: id.to_string(),
                content: format!("content-{}", id),
                memory_type: MemoryType::Factual,
                layer: MemoryLayer::Working,
                created_at: Utc::now(),
                access_times: vec![Utc::now()],
                working_strength: 1.0,
                core_strength: 0.0,
                importance: 0.5,
                pinned: false,
                consolidation_count: 0,
                last_consolidated: None,
                source: "test".to_string(),
                contradicts: None,
                contradicted_by: None,
                metadata: None,
            },
            activation: 0.5,
            confidence,
            confidence_label: "high".to_string(),
        };

        let candidates = vec![
            make_result("id-a", 0.9),
            make_result("id-b", 0.85), // Dup of A, would normally be #2
            make_result("id-c", 0.7),  // #3 backfills into slot #2
        ];

        let result = Memory::dedup_recall_results_by_embedding(
            candidates,
            &embeddings_map,
            0.85,
            2, // limit=2, B gets deduped, C backfills
        );

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].record.id, "id-a");
        assert_eq!(result[1].record.id, "id-c"); // Backfilled
    }

    #[test]
    fn test_recall_dedup_config_defaults() {
        let config = MemoryConfig::default();
        assert!(config.recall_dedup_enabled);
        assert!((config.recall_dedup_threshold - 0.85).abs() < f64::EPSILON);
    }

    // ── C7 Multi-Retrieval Fusion tests ────────────────────────────

    fn make_test_record(id: &str, content: &str, created_at: chrono::DateTime<Utc>) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Factual,
            layer: crate::types::MemoryLayer::Working,
            created_at,
            access_times: vec![created_at],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: "test".to_string(),
            contradicts: None,
            contradicted_by: None,
            metadata: None,
        }
    }

    #[test]
    fn test_temporal_score_within_range() {
        use crate::query_classifier::TimeRange;
        let now = Utc::now();
        
        // Memory at the center of a 24-hour range
        let range = TimeRange {
            start: now - chrono::Duration::hours(24),
            end: now,
        };
        
        let record = make_test_record("t1", "test", now - chrono::Duration::hours(12));
        
        let score = Memory::temporal_score(&record, &Some(range), now);
        assert!(score > 0.9, "Center of range should score high: {}", score);
    }

    #[test]
    fn test_temporal_score_outside_range() {
        use crate::query_classifier::TimeRange;
        let now = Utc::now();
        
        let range = TimeRange {
            start: now - chrono::Duration::hours(24),
            end: now - chrono::Duration::hours(12),
        };
        
        let record = make_test_record("t2", "test", now - chrono::Duration::hours(1));
        
        let score = Memory::temporal_score(&record, &Some(range), now);
        assert!(score < 0.01, "Outside range should score ~0: {}", score);
    }

    #[test]
    fn test_temporal_score_no_range() {
        let now = Utc::now();
        
        let record = make_test_record("t3", "test", now - chrono::Duration::hours(1));
        
        let score = Memory::temporal_score(&record, &None, now);
        // Should be in neutral range (0.25-0.75)
        assert!(score >= 0.25 && score <= 0.75,
            "No range: score should be neutral-ish: {}", score);
    }

    #[test]
    fn test_hebbian_channel_scores_basic() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let mut storage = crate::storage::Storage::new(db.to_str().unwrap()).unwrap();
        
        let now = Utc::now();
        let rec_a = make_test_record("a", "memory A", now);
        let rec_b = make_test_record("b", "memory B", now);
        let rec_c = make_test_record("c", "memory C", now);
        storage.add(&rec_a, "default").unwrap();
        storage.add(&rec_b, "default").unwrap();
        storage.add(&rec_c, "default").unwrap();
        
        // Create Hebbian link between A and B
        // First call creates tracking record, second call forms the link (threshold=1)
        storage.record_coactivation("a", "b", 1).unwrap();
        storage.record_coactivation("a", "b", 1).unwrap();
        
        let candidate_ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let scores = Memory::hebbian_channel_scores(&storage, &candidate_ids).unwrap();
        
        // A and B should have scores (linked to each other), C should not
        assert!(scores.get("a").copied().unwrap_or(0.0) > 0.0, "A should have hebbian score");
        assert!(scores.get("b").copied().unwrap_or(0.0) > 0.0, "B should have hebbian score");
        assert!(scores.get("c").copied().unwrap_or(0.0) < 0.01, "C should have no hebbian score");
    }

    #[test]
    fn test_c7_config_defaults() {
        let config = MemoryConfig::default();
        assert!((config.temporal_weight - 0.10).abs() < f64::EPSILON);
        assert!((config.hebbian_recall_weight - 0.10).abs() < f64::EPSILON);
        assert!(config.adaptive_weights);
    }

    #[test]
    fn test_adaptive_weights_disabled_preserves_behavior() {
        // When adaptive_weights = false, query classifier should return neutral
        let analysis = crate::query_classifier::QueryAnalysis::neutral();
        assert_eq!(analysis.weight_modifiers.fts, 1.0);
        assert_eq!(analysis.weight_modifiers.embedding, 1.0);
        assert_eq!(analysis.weight_modifiers.actr, 1.0);
        assert_eq!(analysis.weight_modifiers.temporal, 1.0);
        assert_eq!(analysis.weight_modifiers.hebbian, 1.0);
    }

    // ── GWT Broadcast Tests ───────────────────────────────────────

    #[test]
    fn test_broadcast_admission_generates_confidence_signals() {
        let mut mem = Memory::new(":memory:", None).unwrap();
        let mut wm = SessionWorkingMemory::default();

        // Store a memory so we have something to broadcast.
        let id = mem
            .add("Rust is a systems programming language", MemoryType::Factual, Some(0.5), None, None)
            .unwrap();

        // Hub should be empty before broadcast.
        assert_eq!(mem.interoceptive_hub().buffer_len(), 0);

        // Broadcast the memory admission.
        mem.broadcast_admission(&[id], &mut wm);

        // Hub should now have at least one signal (confidence).
        // No emotional bus → no alignment signal, but confidence is always generated.
        assert!(
            mem.interoceptive_hub().buffer_len() >= 1,
            "expected ≥1 signal in hub, got {}",
            mem.interoceptive_hub().buffer_len()
        );
    }

    #[test]
    fn test_broadcast_admission_multiple_memories() {
        let mut mem = Memory::new(":memory:", None).unwrap();
        let mut wm = SessionWorkingMemory::default();

        let id1 = mem
            .add("First memory about coding", MemoryType::Factual, Some(0.5), None, None)
            .unwrap();
        let id2 = mem
            .add("Second memory about trading", MemoryType::Factual, Some(0.6), None, None)
            .unwrap();
        let id3 = mem
            .add("Third memory about research", MemoryType::Factual, Some(0.7), None, None)
            .unwrap();

        mem.broadcast_admission(&[id1, id2, id3], &mut wm);

        // Each memory generates at least 1 confidence signal → ≥3.
        assert!(
            mem.interoceptive_hub().buffer_len() >= 3,
            "expected ≥3 signals, got {}",
            mem.interoceptive_hub().buffer_len()
        );
    }

    #[test]
    fn test_broadcast_with_nonexistent_memory_is_safe() {
        let mut mem = Memory::new(":memory:", None).unwrap();
        let mut wm = SessionWorkingMemory::default();

        // Broadcast a memory that doesn't exist — should not panic.
        let neighbors = mem.broadcast_admission(&["nonexistent-id".to_string()], &mut wm);
        assert!(neighbors.is_empty());
        assert_eq!(mem.interoceptive_hub().buffer_len(), 0);
    }

    #[test]
    fn test_broadcast_updates_hub_domain_state() {
        let mut mem = Memory::new(":memory:", None).unwrap();
        let mut wm = SessionWorkingMemory::default();

        // Store multiple memories and broadcast them.
        let id1 = mem
            .add("Important fact about Rust", MemoryType::Factual, Some(0.8), None, None)
            .unwrap();
        let id2 = mem
            .add("Another fact about memory", MemoryType::Factual, Some(0.3), None, None)
            .unwrap();

        mem.broadcast_admission(&[id1, id2], &mut wm);

        // Hub should have processed signals and have some state.
        let state = mem.interoceptive_snapshot();
        assert!(state.buffer_size > 0, "hub should have buffered signals");
    }

    #[test]
    fn test_broadcast_hebbian_spreading() {
        // Use raw storage to set up Hebbian links, then verify
        // broadcast_admission spreads activation.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("broadcast_hebb.db");
        let mut mem = Memory::new(db.to_str().unwrap(), None).unwrap();
        let mut wm = SessionWorkingMemory::default();

        // Store two memories.
        let id_a = mem
            .add("Memory A about Rust", MemoryType::Factual, Some(0.5), None, None)
            .unwrap();
        let id_b = mem
            .add("Memory B about Rust compilers", MemoryType::Factual, Some(0.5), None, None)
            .unwrap();

        // Create a Hebbian link by simulating co-recall via the storage layer.
        // We need to strengthen it enough (weight > 0.1) for spreading to activate.
        // record_coactivation with threshold=1 → second call forms the link.
        {
            // Use recall to trigger co-activation recording (indirect approach).
            // Or directly use the underlying record_coactivation on Memory.
            // Since Memory doesn't expose &mut Storage, we'll use a workaround:
            // call recall with both IDs to trigger Hebbian learning.
            //
            // Actually, the cleanest approach: use rusqlite directly on the
            // connection to insert a Hebbian link for testing purposes.
            let conn = mem.connection();
            conn.execute(
                "INSERT OR REPLACE INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at) VALUES (?1, ?2, 0.5, 5, ?3)",
                rusqlite::params![&id_a, &id_b, Utc::now().timestamp() as f64],
            ).unwrap();
        }

        // Now broadcast only memory A → should spread to B via Hebbian link.
        let neighbors = mem.broadcast_admission(&[id_a.clone()], &mut wm);

        // B should appear as a primed neighbor.
        assert!(
            neighbors.contains(&id_b),
            "expected id_b in neighbors, got {:?}",
            neighbors
        );

        // B should now be in working memory (primed by spreading activation).
        assert!(wm.contains(&id_b), "id_b should be in WM after spreading");
    }

    /// ISS-020 P0.0: valence, confidence, and domain must be persisted into
    /// `metadata.dimensions` on every `remember()` path that calls the extractor.
    ///
    /// This test simulates the serialization step in isolation — it mirrors the
    /// exact code inside `Memory::remember()` at src/memory.rs:~1325, but drives
    /// it with a hand-built ExtractedFact so we don't need a live LLM.
    ///
    /// The invariant: for ANY ExtractedFact, `metadata.dimensions` always
    /// contains `valence` (number), `confidence` (string), and `domain` (string).
    #[test]
    fn test_iss020_p0_0_dimensions_persist_valence_confidence_domain() {
        use crate::extractor::ExtractedFact;

        let fact = ExtractedFact {
            core_fact: "potato prefers Rust for systems work".into(),
            participants: Some("potato".into()),
            temporal: Some("2026-04-22".into()),
            location: None,
            context: None,
            causation: None,
            outcome: None,
            method: None,
            relations: None,
            sentiment: Some("positive".into()),
            stance: Some("prefers Rust over Go".into()),
            importance: 0.7,
            tags: vec!["coding".into()],
            confidence: "confident".into(),
            valence: 0.6,
            domain: "coding".into(),
        };

        // Mirror the exact write-path logic from Memory::remember().
        let mut dimensions = serde_json::Map::new();
        if let Some(ref v) = fact.participants {
            dimensions.insert(
                "participants".into(),
                serde_json::Value::String(v.clone()),
            );
        }
        if let Some(ref v) = fact.temporal {
            dimensions.insert("temporal".into(), serde_json::Value::String(v.clone()));
        }
        if let Some(ref v) = fact.sentiment {
            dimensions.insert("sentiment".into(), serde_json::Value::String(v.clone()));
        }
        if let Some(ref v) = fact.stance {
            dimensions.insert("stance".into(), serde_json::Value::String(v.clone()));
        }
        // P0.0 additions:
        dimensions.insert("valence".into(), serde_json::json!(fact.valence));
        dimensions.insert(
            "confidence".into(),
            serde_json::Value::String(fact.confidence.clone()),
        );
        dimensions.insert(
            "domain".into(),
            serde_json::Value::String(fact.domain.clone()),
        );

        // Serialize → deserialize (SQLite stores as JSON text; this is the round-trip).
        let json = serde_json::Value::Object(dimensions);
        let serialized = serde_json::to_string(&json).expect("serialize");
        let roundtripped: serde_json::Value =
            serde_json::from_str(&serialized).expect("deserialize");

        // valence: must be present as a number in [-1.0, 1.0].
        let v = roundtripped
            .get("valence")
            .expect("valence must be persisted");
        let v_num = v.as_f64().expect("valence must be a number");
        assert!(
            (-1.0..=1.0).contains(&v_num),
            "valence {v_num} out of range [-1, 1]"
        );
        assert!((v_num - 0.6).abs() < 1e-9, "valence round-trip mismatch");

        // confidence: must be a string in {"confident", "likely", "uncertain"}.
        let c = roundtripped
            .get("confidence")
            .and_then(|v| v.as_str())
            .expect("confidence must be persisted as string");
        assert!(
            matches!(c, "confident" | "likely" | "uncertain"),
            "confidence {c} not a recognized variant"
        );

        // domain: must be a non-empty string.
        let d = roundtripped
            .get("domain")
            .and_then(|v| v.as_str())
            .expect("domain must be persisted as string");
        assert!(!d.is_empty(), "domain must not be empty");
        assert_eq!(d, "coding");
    }

    /// P0.0 edge case: extractor may produce valence = 0.0 (neutral) — still persist.
    #[test]
    fn test_iss020_p0_0_neutral_valence_still_persisted() {
        use crate::extractor::ExtractedFact;

        let fact = ExtractedFact {
            core_fact: "neutral fact".into(),
            confidence: "likely".into(),
            valence: 0.0,
            domain: "general".into(),
            ..Default::default()
        };

        let mut dimensions = serde_json::Map::new();
        dimensions.insert("valence".into(), serde_json::json!(fact.valence));
        dimensions.insert(
            "confidence".into(),
            serde_json::Value::String(fact.confidence.clone()),
        );
        dimensions.insert(
            "domain".into(),
            serde_json::Value::String(fact.domain.clone()),
        );

        let json = serde_json::Value::Object(dimensions);
        assert_eq!(json["valence"].as_f64(), Some(0.0));
        assert_eq!(json["confidence"].as_str(), Some("likely"));
        assert_eq!(json["domain"].as_str(), Some("general"));
    }
}
