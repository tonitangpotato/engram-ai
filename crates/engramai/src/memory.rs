//! Main Memory API — simplified interface to Engram's cognitive models.

use chrono::Utc;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use uuid::Uuid;

use crate::bus::EmotionalBus;
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
    emotional_bus: Option<EmotionalBus>,
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
            emotional_bus: None,
            embedding,
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
        };
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        
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
            emotional_bus: None,
            embedding: Some(embedding),
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
        };
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        
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
            emotional_bus: None,
            embedding,
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
        };
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        
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
    pub fn with_emotional_bus(
        path: &str,
        workspace_dir: &str,
        config: Option<MemoryConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let storage = Storage::new(path)?;
        let config = config.unwrap_or_default();
        let created_at = Utc::now();
        
        // Create Emotional Bus using storage's connection
        let emotional_bus = Some(EmotionalBus::new(workspace_dir, storage.connection())?);
        
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
            emotional_bus,
            embedding,
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
        };
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        
        Ok(mem)
    }
    
    /// Get a reference to the Emotional Bus, if attached.
    pub fn emotional_bus(&self) -> Option<&EmotionalBus> {
        self.emotional_bus.as_ref()
    }
    
    /// Get a mutable reference to the Emotional Bus, if attached.
    pub fn emotional_bus_mut(&mut self) -> Option<&mut EmotionalBus> {
        self.emotional_bus.as_mut()
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
                        log::info!("  → [{}] (imp={:.1}) {}", fact.memory_type, fact.importance,
                            fact.content.chars().take(80).collect::<String>());
                    }
                    for fact in facts {
                        // Convert extracted memory_type string to MemoryType enum
                        let fact_type = Self::parse_memory_type(&fact.memory_type)
                            .unwrap_or(memory_type);
                        
                        // Use extracted importance, fall back to provided or type default
                        let fact_importance = Some(fact.importance);
                        
                        // Store each extracted fact separately
                        last_id = self.add_raw(
                            &fact.content,
                            fact_type,
                            fact_importance,
                            source,
                            metadata.clone(),
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
        let importance = if let Some(ref bus) = self.emotional_bus {
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
                    self.storage.merge_memory_into(&existing_id, importance)?;
                    
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
        if let Some(ref bus) = self.emotional_bus {
            bus.process_interaction(self.storage.connection(), content, emotion, domain)?;
        }
        
        Ok(id)
    }

    /// Retrieve relevant memories using ACT-R activation-based retrieval.
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
            
            // Score each candidate with combined FTS + embedding + ACT-R + entity
            // Weights are configurable via MemoryConfig, runtime-normalized to sum to 1.0
            let raw_fts_weight = self.config.fts_weight;
            let raw_emb_weight = self.config.embedding_weight;
            let raw_actr_weight = self.config.actr_weight;
            let raw_entity_weight = entity_w;
            
            // Runtime normalization — always divide by sum (handles any user config)
            let total_weight = raw_fts_weight + raw_emb_weight + raw_actr_weight + raw_entity_weight;
            let (fts_weight, emb_weight, actr_weight, ent_weight) = if total_weight > 0.0 {
                (
                    raw_fts_weight / total_weight,
                    raw_emb_weight / total_weight,
                    raw_actr_weight / total_weight,
                    raw_entity_weight / total_weight,
                )
            } else {
                (0.25, 0.25, 0.25, 0.25)
            };
            
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
                    
                    // Combined: FTS + embedding + ACT-R + entity
                    let combined_score = (fts_weight * fts_score)
                        + (emb_weight * embedding_score as f64)
                        + (actr_weight * activation_normalized)
                        + (ent_weight * entity_score);
                    
                    (record, combined_score, activation)
                })
                .collect();

            // Sort by combined score descending
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

            // Take top-k and compute confidence
            let results: Vec<_> = scored
                .into_iter()
                .take(limit)
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
                        record,
                        activation,
                        confidence,
                        confidence_label,
                    }
                })
                .filter(|r| r.confidence >= min_conf)
                .collect();

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
}
