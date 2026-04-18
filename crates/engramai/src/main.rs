//! Engram CLI — command-line interface for multi-agent memory.
//!
//! Usage:
//!   engram store "content" --ns trading --type factual --importance 0.8
//!   engram recall "query" --ns "*" --limit 5 --json
//!   engram stats --ns trading
//!   engram consolidate --ns trading
//!   engram grant agent-id --ns namespace --perm read
//!   engram revoke agent-id --ns namespace
//!   engram bus trends
//!   engram bus suggest
//!   engram bus log-outcome check_email --positive

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use engramai::{Memory, MemoryConfig, MemoryType, Permission, EmotionalBus, EmbeddingConfig, AnthropicExtractor, OllamaExtractor};
use engramai::compiler::{
    self,
    api::MaintenanceApi,
    compilation::MemorySnapshot,
    conflict::ConflictDetector,
    decay::DecayEngine,
    discovery::TopicDiscovery,
    export::{ExportEngine, ExportOutput},
    health::HealthAuditor,
    import::{ImportPipeline, MarkdownImporter},
    llm::NoopProvider,
    privacy::{AccessContext, PrivacyGuard},
    storage::{KnowledgeStore, SqliteKnowledgeStore},
    types::*,
};

/// Engram — Neuroscience-grounded memory system for AI agents.
#[derive(Parser)]
#[command(name = "engram")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to SQLite database file
    #[arg(short, long, env = "ENGRAM_DB", default_value = "engram.db")]
    database: PathBuf,
    
    /// Agent ID for this session (used for ACL)
    #[arg(short, long, env = "ENGRAM_AGENT_ID")]
    agent_id: Option<String>,
    
    /// Workspace directory for Emotional Bus (SOUL.md, HEARTBEAT.md, etc.)
    #[arg(short, long, env = "ENGRAM_WORKSPACE")]
    workspace: Option<PathBuf>,
    
    /// Ollama embedding model (default: nomic-embed-text)
    #[arg(long, env = "ENGRAM_EMBEDDING_MODEL")]
    embedding_model: Option<String>,
    
    /// Ollama host URL (default: http://localhost:11434)
    #[arg(long, env = "ENGRAM_EMBEDDING_HOST")]
    embedding_host: Option<String>,
    
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize config file (~/.config/engram/config.json)
    Init {
        /// Force overwrite existing config
        #[arg(long, short = 'f')]
        force: bool,
    },
    
    /// Store a new memory
    Store {
        /// Memory content
        content: String,
        
        /// Namespace to store in
        #[arg(long, short = 'n', default_value = "default")]
        ns: String,
        
        /// Memory type
        #[arg(long, short = 't', default_value = "factual")]
        r#type: MemoryTypeArg,
        
        /// Importance score (0.0-1.0)
        #[arg(long, short = 'i')]
        importance: Option<f64>,
        
        /// Source identifier
        #[arg(long, short = 's')]
        source: Option<String>,
        
        /// Emotional valence (-1.0 to 1.0)
        #[arg(long, short = 'e')]
        emotion: Option<f64>,
        
        /// Domain for emotional tracking
        #[arg(long)]
        domain: Option<String>,
        
        /// Use LLM extractor to extract facts (ollama, anthropic)
        #[arg(long, env = "ENGRAM_EXTRACTOR")]
        extractor: Option<ExtractorArg>,
        
        /// Ollama model for extraction (default: llama3.2:3b)
        #[arg(long, env = "ENGRAM_EXTRACTOR_MODEL")]
        extractor_model: Option<String>,
        
        /// Anthropic auth token (API key or OAuth token)
        #[arg(long, env = "ANTHROPIC_API_KEY")]
        auth_token: Option<String>,
        
        /// Use OAuth mode for Anthropic (Claude Max)
        #[arg(long)]
        oauth: bool,
    },
    
    /// Recall memories by query
    Recall {
        /// Search query
        query: String,
        
        /// Namespace to search (use "*" for all)
        #[arg(long, short = 'n', default_value = "default")]
        ns: String,
        
        /// Maximum number of results
        #[arg(long, short = 'l', default_value = "5")]
        limit: usize,
        
        /// Minimum confidence threshold
        #[arg(long, short = 'c')]
        min_confidence: Option<f64>,
        
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// Fetch the N most recent memories (chronological, no query needed)
    RecallRecent {
        /// Maximum number of results
        #[arg(long, short = 'l', default_value = "50")]
        limit: usize,

        /// Namespace to search (use "*" for all)
        #[arg(long, short = 'n', default_value = "default")]
        ns: String,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// Show memory statistics
    Stats {
        /// Namespace to show stats for (use "*" for all)
        #[arg(long, short = 'n')]
        ns: Option<String>,
        
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// Run memory consolidation cycle
    Consolidate {
        /// Namespace to consolidate (omit for all)
        #[arg(long, short = 'n')]
        ns: Option<String>,
        
        /// Simulated days of consolidation
        #[arg(long, short = 'd', default_value = "1.0")]
        days: f64,
    },
    
    /// Grant access permission to an agent
    Grant {
        /// Agent ID to grant permission to
        agent_id: String,
        
        /// Namespace to grant access to
        #[arg(long, short = 'n')]
        ns: String,
        
        /// Permission level (read, write, admin)
        #[arg(long, short = 'p', default_value = "read")]
        perm: PermissionArg,
    },
    
    /// Revoke access permission from an agent
    Revoke {
        /// Agent ID to revoke permission from
        agent_id: String,
        
        /// Namespace to revoke access from
        #[arg(long, short = 'n')]
        ns: String,
    },
    
    /// List permissions for an agent
    Permissions {
        /// Agent ID to list permissions for
        agent_id: String,
        
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// Pin a memory (prevent decay)
    Pin {
        /// Memory ID to pin
        memory_id: String,
    },
    
    /// Unpin a memory (allow decay)
    Unpin {
        /// Memory ID to unpin
        memory_id: String,
    },
    
    /// Delete a specific memory
    Forget {
        /// Memory ID to delete
        memory_id: String,
    },
    
    /// Update an existing memory's content
    Update {
        /// Memory ID to update
        memory_id: String,
        
        /// New content
        new_content: String,
        
        /// Reason for update (stored in metadata)
        #[arg(long, short = 'r', default_value = "manual update")]
        reason: String,
    },
    
    /// Export memories to JSON file
    Export {
        /// Output file path
        path: String,
        
        /// Namespace to export (omit for all)
        #[arg(long, short = 'n')]
        ns: Option<String>,
    },
    
    /// Recall associated memories
    RecallAssociated {
        /// Optional query to filter associated memories
        query: Option<String>,
        
        /// Maximum number of results
        #[arg(long, short = 'l', default_value = "5")]
        limit: usize,
        
        /// Minimum confidence threshold
        #[arg(long, short = 'c', default_value = "0.0")]
        min_confidence: f64,
        
        /// Namespace to search
        #[arg(long, short = 'n')]
        ns: Option<String>,
        
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// Get a specific memory by ID
    Get {
        /// Memory ID
        memory_id: String,
        
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// List all memories
    List {
        /// Maximum number of results
        #[arg(long, short = 'l', default_value = "20")]
        limit: usize,
        
        /// Namespace to list
        #[arg(long, short = 'n')]
        ns: Option<String>,
        
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// Apply reward signal to recent memories
    Reward {
        /// Feedback text (positive/negative sentiment detected)
        feedback: String,
        
        /// Number of recent memories to affect
        #[arg(long, short = 'n', default_value = "3")]
        recent: usize,
    },
    
    /// Emotional Bus commands
    Bus {
        #[command(subcommand)]
        action: BusAction,
    },
    
    // === Phase 3: Cross-Agent Intelligence ===
    
    /// Subscribe to namespace notifications
    Subscribe {
        /// Agent ID to subscribe
        agent_id: String,
        
        /// Namespace to watch ("*" for all)
        #[arg(long, short = 'n')]
        ns: String,
        
        /// Minimum importance to trigger notification (0.0-1.0)
        #[arg(long, short = 'i', default_value = "0.8")]
        min_importance: f64,
    },
    
    /// Unsubscribe from namespace notifications
    Unsubscribe {
        /// Agent ID to unsubscribe
        agent_id: String,
        
        /// Namespace to stop watching
        #[arg(long, short = 'n')]
        ns: String,
    },
    
    /// Check pending notifications for an agent
    Notifications {
        /// Agent ID to check notifications for
        agent_id: String,
        
        /// Just peek without marking as read
        #[arg(long)]
        peek: bool,
        
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// Discover cross-namespace associations
    CrossLinks {
        /// First namespace
        #[arg(long)]
        ns_a: String,
        
        /// Second namespace
        #[arg(long)]
        ns_b: String,
        
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// Recall with cross-namespace associations
    RecallAssoc {
        /// Search query
        query: String,
        
        /// Namespace to search (use "*" for all with cross-links)
        #[arg(long, short = 'n', default_value = "*")]
        ns: String,
        
        /// Maximum number of results
        #[arg(long, short = 'l', default_value = "5")]
        limit: usize,
        
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// List subscriptions for an agent
    Subscriptions {
        /// Agent ID to list subscriptions for
        agent_id: String,
        
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    // === Embedding Commands ===
    
    /// Reindex embeddings for all memories without embeddings
    Reindex {
        /// Show progress during reindexing
        #[arg(long, short = 'p')]
        progress: bool,
    },
    
    /// Show embedding status
    EmbeddingStatus {
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    // === Entity Management ===
    
    /// Manage entity index
    Entities {
        #[command(subcommand)]
        command: Option<EntityCommand>,
        
        /// Filter by entity type (project, person, technology, etc.)
        #[arg(long, short = 't')]
        entity_type: Option<String>,
    },

    // === Knowledge Synthesis ===

    /// Run knowledge synthesis (discover clusters, gate check, generate insights)
    Synthesize {
        /// Dry run: show clusters and gate decisions without making changes
        #[arg(long)]
        dry_run: bool,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// List synthesized insights
    Insights {
        /// Maximum number of results
        #[arg(long, short = 'l', default_value = "20")]
        limit: usize,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// Show details and provenance of a specific insight
    Insight {
        /// Insight memory ID
        id: String,

        /// Show source memories
        #[arg(long)]
        sources: bool,

        /// Show provenance chain
        #[arg(long)]
        provenance: bool,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// Run unified sleep cycle (consolidation + synthesis)
    Sleep {
        /// Namespace (omit for all)
        #[arg(long, short = 'n')]
        ns: Option<String>,

        /// Simulated days of consolidation
        #[arg(long, short = 'd', default_value = "7.0")]
        days: f64,

        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },

    /// Reverse a synthesis (restore sources, archive insight)
    Unsynthesise {
        /// Insight memory ID to reverse
        id: String,
    },

    /// Knowledge compiler — compile, query, maintain knowledge
    #[command(subcommand)]
    Knowledge(KnowledgeCommand),
}

#[derive(Subcommand)]
enum BusAction {
    /// Show emotional trends by domain
    Trends {
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// Show suggested SOUL/HEARTBEAT updates
    Suggest {
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// Log a behavior outcome
    LogOutcome {
        /// Action name (e.g., "check_email", "run_consolidation")
        action: String,
        
        /// Mark outcome as positive
        #[arg(long, conflicts_with = "negative")]
        positive: bool,
        
        /// Mark outcome as negative
        #[arg(long, conflicts_with = "positive")]
        negative: bool,
    },
    
    /// Show behavior statistics
    BehaviorStats {
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    
    /// Record an emotional event
    RecordEmotion {
        /// Domain (e.g., "coding", "communication")
        domain: String,
        
        /// Emotional valence (-1.0 to 1.0)
        #[arg(long, short = 'v')]
        valence: f64,
    },
    
    /// Check drive alignment for content
    Alignment {
        /// Content to check alignment for
        content: String,
        
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum EntityCommand {
    /// Run backfill on existing memories
    Backfill {
        /// Batch size (default: 500)
        #[arg(long, default_value = "500")]
        batch_size: usize,
    },
    /// Purge garbage entities (false-positive persons, orphans)
    Purge,
    /// Show entity statistics
    Stats,
    /// List entities (default view)
    List {
        /// Filter by entity type
        #[arg(long, short = 't')]
        entity_type: Option<String>,
        /// Max entities to show
        #[arg(long, default_value = "20")]
        limit: usize,
    },
}

#[derive(Clone, ValueEnum)]
enum MemoryTypeArg {
    Factual,
    Episodic,
    Relational,
    Emotional,
    Procedural,
    Opinion,
    Causal,
}

impl From<MemoryTypeArg> for MemoryType {
    fn from(arg: MemoryTypeArg) -> Self {
        match arg {
            MemoryTypeArg::Factual => MemoryType::Factual,
            MemoryTypeArg::Episodic => MemoryType::Episodic,
            MemoryTypeArg::Relational => MemoryType::Relational,
            MemoryTypeArg::Emotional => MemoryType::Emotional,
            MemoryTypeArg::Procedural => MemoryType::Procedural,
            MemoryTypeArg::Opinion => MemoryType::Opinion,
            MemoryTypeArg::Causal => MemoryType::Causal,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum PermissionArg {
    Read,
    Write,
    Admin,
}

impl From<PermissionArg> for Permission {
    fn from(arg: PermissionArg) -> Self {
        match arg {
            PermissionArg::Read => Permission::Read,
            PermissionArg::Write => Permission::Write,
            PermissionArg::Admin => Permission::Admin,
        }
    }
}

/// LLM extractor backend for memory extraction.
#[derive(Clone, ValueEnum)]
enum ExtractorArg {
    /// Use local Ollama for extraction (default model: llama3.2:3b)
    Ollama,
    /// Use Anthropic Claude API for extraction (default model: claude-haiku-4-5-20251001)
    Anthropic,
}

#[derive(Clone, Debug, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
    Md,
}

#[derive(Subcommand)]
enum KnowledgeCommand {
    /// Search compiled knowledge topics
    Query {
        /// Search term
        search: String,
        /// Max results
        #[arg(long, short = 'l', default_value = "10")]
        limit: usize,
        /// Output format
        #[arg(long, short = 'f', default_value = "text")]
        format: OutputFormat,
    },
    /// Inspect a specific topic page
    Inspect {
        /// Topic ID
        topic_id: String,
        /// Show source memories
        #[arg(long)]
        sources: bool,
        /// Show conflicts
        #[arg(long)]
        conflicts: bool,
    },
    /// Export knowledge to file
    Export {
        /// Output path
        #[arg(long, short = 'o', default_value = "knowledge-export")]
        output: String,
        /// Format: json, md
        #[arg(long, short = 'f', default_value = "md")]
        format: OutputFormat,
    },
    /// Import knowledge from file/directory
    Import {
        /// Path to import from
        path: String,
    },
    /// Health report
    Health {
        /// Scope: all, stale, conflicted
        #[arg(long, default_value = "all")]
        scope: String,
        /// Output format
        #[arg(long, short = 'f', default_value = "text")]
        format: OutputFormat,
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    /// Evaluate or apply decay
    Decay {
        /// Evaluate only (don't apply)
        #[arg(long)]
        evaluate: bool,
        /// Apply decay
        #[arg(long)]
        apply: bool,
        /// Target specific topic
        #[arg(long)]
        topic: Option<String>,
    },
    /// Scan for conflicts between topics
    Conflicts {
        /// Scan for new conflicts
        #[arg(long)]
        scan: bool,
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
    },
    /// Audit links and duplicates
    Audit {
        /// Check link integrity
        #[arg(long)]
        links: bool,
        /// Check for duplicates
        #[arg(long)]
        duplicates: bool,
        /// Auto-repair broken links
        #[arg(long)]
        repair: bool,
    },
    /// Privacy controls
    Privacy {
        /// Set privacy level for a topic
        #[arg(long)]
        set_level: Option<String>,
        /// Privacy level (public, private, sensitive, restricted)
        #[arg(long)]
        level: Option<String>,
        /// Show audit log
        #[arg(long)]
        audit_log: bool,
    },
    /// Compile memories into knowledge topics
    Compile {
        /// Topic ID to recompile (omit for auto-discovery)
        #[arg(long)]
        topic: Option<String>,
        /// Dry run
        #[arg(long)]
        dry_run: bool,
    },
    /// List all compiled topic pages
    List {
        /// Output as JSON
        #[arg(long, short = 'j')]
        json: bool,
        /// Max results
        #[arg(long, short = 'l', default_value = "50")]
        limit: usize,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logger
    env_logger::init();
    
    let cli = Cli::parse();
    
    let db_path = cli.database.to_str().ok_or("invalid database path")?;
    
    // Build embedding config from CLI args
    let mut embedding_config = EmbeddingConfig::default();
    if let Some(ref model) = cli.embedding_model {
        embedding_config.model = model.clone();
    }
    if let Some(ref host) = cli.embedding_host {
        embedding_config.host = host.clone();
    }
    
    // Build memory config with embedding settings
    let mut mem_config = MemoryConfig::default();
    mem_config.embedding = embedding_config;
    
    // Create Memory with or without Emotional Bus
    let mut mem = if let Some(ref workspace) = cli.workspace {
        let ws_path = workspace.to_str().ok_or("invalid workspace path")?;
        Memory::with_emotional_bus(db_path, ws_path, Some(mem_config))?
    } else {
        Memory::new(db_path, Some(mem_config))?
    };
    
    if let Some(agent_id) = &cli.agent_id {
        mem.set_agent_id(agent_id);
    }
    
    match cli.command {
        Commands::Init { force } => {
            // Create config directory
            let config_dir = dirs::config_dir()
                .ok_or("Could not determine config directory")?
                .join("engram");
            
            std::fs::create_dir_all(&config_dir)?;
            
            let config_path = config_dir.join("config.json");
            
            // Check if config already exists
            if config_path.exists() && !force {
                eprintln!("Config file already exists at: {}", config_path.display());
                eprintln!("Use --force to overwrite.");
                std::process::exit(1);
            }
            
            // Interactive prompts
            use std::io::{self, Write};
            
            fn prompt(question: &str, default: &str) -> String {
                print!("{} [{}]: ", question, default);
                io::stdout().flush().unwrap();
                let mut input = String::new();
                io::stdin().read_line(&mut input).unwrap();
                let trimmed = input.trim();
                if trimmed.is_empty() {
                    default.to_string()
                } else {
                    trimmed.to_string()
                }
            }
            
            println!("Engram Configuration Setup");
            println!("==========================\n");
            
            // Embedding provider
            let embedding_provider = prompt("Embedding provider (ollama/none)", "ollama");
            
            // Extractor provider
            let extractor_provider = prompt("Extractor provider (anthropic/ollama/none)", "anthropic");
            
            // Build config JSON
            let mut config = serde_json::json!({});
            
            if embedding_provider != "none" {
                let embedding_model = if embedding_provider == "ollama" {
                    prompt("Embedding model", "nomic-embed-text")
                } else {
                    "nomic-embed-text".to_string()
                };
                let embedding_host = if embedding_provider == "ollama" {
                    prompt("Ollama host", "http://localhost:11434")
                } else {
                    "http://localhost:11434".to_string()
                };
                
                config["embedding"] = serde_json::json!({
                    "provider": embedding_provider,
                    "model": embedding_model,
                    "host": embedding_host
                });
            }
            
            if extractor_provider != "none" {
                let extractor_model = match extractor_provider.as_str() {
                    "anthropic" => prompt("Extractor model", "claude-haiku-4-5-20251001"),
                    "ollama" => prompt("Extractor model", "llama3.2:3b"),
                    _ => "claude-haiku-4-5-20251001".to_string()
                };
                
                let mut extractor_config = serde_json::json!({
                    "provider": extractor_provider,
                    "model": extractor_model
                });
                
                if extractor_provider == "ollama" {
                    let ollama_host = prompt("Ollama host for extraction", "http://localhost:11434");
                    extractor_config["host"] = serde_json::json!(ollama_host);
                }
                
                config["extractor"] = extractor_config;
            }
            
            // Write config file
            let config_str = serde_json::to_string_pretty(&config)?;
            std::fs::write(&config_path, &config_str)?;
            
            println!("\n✅ Config written to: {}", config_path.display());
            println!("\nConfig contents:");
            println!("{}", config_str);
            
            // Remind about auth
            if extractor_provider == "anthropic" {
                println!("\n⚠️  Remember to set your Anthropic auth token:");
                println!("   export ANTHROPIC_API_KEY=sk-ant-...");
                println!("   # or for Claude Max:");
                println!("   export ANTHROPIC_AUTH_TOKEN=sk-ant-oat01-...");
            }
            
            return Ok(());
        }
        
        Commands::Store { content, ns, r#type, importance, source, emotion, domain, extractor, extractor_model, auth_token, oauth } => {
            // Set up extractor if requested
            if let Some(ext) = extractor {
                match ext {
                    ExtractorArg::Ollama => {
                        let model = extractor_model.as_deref().unwrap_or("llama3.2:3b");
                        let host = cli.embedding_host.as_deref().unwrap_or("http://localhost:11434");
                        let ollama_extractor = OllamaExtractor::with_host(model, host);
                        mem.set_extractor(Box::new(ollama_extractor));
                        log::info!("Using Ollama extractor with model: {}", model);
                    }
                    ExtractorArg::Anthropic => {
                        let token = auth_token.ok_or("Anthropic extractor requires --auth-token or ANTHROPIC_API_KEY")?;
                        let anthropic_extractor = AnthropicExtractor::new(&token, oauth);
                        mem.set_extractor(Box::new(anthropic_extractor));
                        log::info!("Using Anthropic extractor (oauth: {})", oauth);
                    }
                }
            }
            
            // If emotion is provided, use add_with_emotion
            let id = if let (Some(em), Some(dom)) = (emotion, domain.as_ref()) {
                mem.add_with_emotion(
                    &content,
                    r#type.into(),
                    importance,
                    source.as_deref(),
                    None,
                    Some(&ns),
                    em,
                    dom,
                )?
            } else {
                mem.add_to_namespace(
                    &content,
                    r#type.into(),
                    importance,
                    source.as_deref(),
                    None,
                    Some(&ns),
                )?
            };
            
            // Handle empty ID (extractor found nothing worth storing)
            if id.is_empty() {
                println!("(no facts extracted)");
            } else {
                println!("{}", id);
            }
        }
        
        Commands::Recall { query, ns, limit, min_confidence, json } => {
            let ns_opt = if ns == "default" { None } else { Some(ns.as_str()) };
            let results = mem.recall_from_namespace(&query, limit, None, min_confidence, ns_opt)?;
            
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                if results.is_empty() {
                    println!("No memories found.");
                } else {
                    for r in &results {
                        println!("[{}] ({:.2}) {}", r.record.id, r.confidence, r.record.content);
                        if !r.record.source.is_empty() {
                            println!("    source: {}", r.record.source);
                        }
                    }
                }
            }
        }
        
        Commands::RecallRecent { limit, ns, json } => {
            let ns_opt = if ns == "default" { None } else { Some(ns.as_str()) };
            let records = mem.recall_recent(limit, ns_opt)?;
            
            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else {
                if records.is_empty() {
                    println!("No recent memories.");
                } else {
                    println!("Recent {} memories (newest first):", records.len());
                    for r in &records {
                        let age = chrono::Utc::now() - r.created_at;
                        let age_str = if age.num_hours() < 1 {
                            format!("{}m ago", age.num_minutes())
                        } else if age.num_hours() < 24 {
                            format!("{}h ago", age.num_hours())
                        } else {
                            format!("{}d ago", age.num_days())
                        };
                        println!("[{}] ({}) [{}] {}", age_str, r.memory_type, r.layer, r.content);
                    }
                }
            }
        }

        Commands::Stats { ns, json } => {
            let stats = mem.stats_ns(ns.as_deref())?;
            
            if json {
                println!("{}", serde_json::to_string_pretty(&stats)?);
            } else {
                println!("Total memories: {}", stats.total_memories);
                println!("Pinned: {}", stats.pinned);
                println!("Uptime: {:.2} hours", stats.uptime_hours);
                println!("\nBy type:");
                for (type_name, type_stats) in &stats.by_type {
                    println!("  {}: {} (avg strength: {:.3}, avg importance: {:.3})",
                        type_name, type_stats.count, type_stats.avg_strength, type_stats.avg_importance);
                }
                println!("\nBy layer:");
                for (layer_name, layer_stats) in &stats.by_layer {
                    println!("  {}: {} (avg working: {:.3}, avg core: {:.3})",
                        layer_name, layer_stats.count, layer_stats.avg_working, layer_stats.avg_core);
                }
            }
        }
        
        Commands::Consolidate { ns, days } => {
            mem.consolidate_namespace(days, ns.as_deref())?;
            println!("Consolidation complete ({} days simulated)", days);
        }
        
        Commands::Grant { agent_id, ns, perm } => {
            let perm_str = match perm {
                PermissionArg::Read => "read",
                PermissionArg::Write => "write",
                PermissionArg::Admin => "admin",
            };
            mem.grant(&agent_id, &ns, perm.into())?;
            println!("Granted {} permission to {} on namespace {}", perm_str, agent_id, ns);
        }
        
        Commands::Revoke { agent_id, ns } => {
            mem.revoke(&agent_id, &ns)?;
            println!("Revoked permission from {} on namespace {}", agent_id, ns);
        }
        
        Commands::Permissions { agent_id, json } => {
            let perms = mem.list_permissions(&agent_id)?;
            
            if json {
                println!("{}", serde_json::to_string_pretty(&perms)?);
            } else {
                if perms.is_empty() {
                    println!("No permissions found for {}", agent_id);
                } else {
                    println!("Permissions for {}:", agent_id);
                    for p in &perms {
                        println!("  {} on {} (granted by {} at {})",
                            p.permission, p.namespace, p.granted_by, p.created_at);
                    }
                }
            }
        }
        
        Commands::Pin { memory_id } => {
            mem.pin(&memory_id)?;
            println!("Pinned memory {}", memory_id);
        }
        
        Commands::Unpin { memory_id } => {
            mem.unpin(&memory_id)?;
            println!("Unpinned memory {}", memory_id);
        }
        
        Commands::Forget { memory_id } => {
            mem.forget(Some(&memory_id), None)?;
            println!("Deleted memory {}", memory_id);
        }
        
        Commands::Reward { feedback, recent } => {
            mem.reward(&feedback, recent)?;
            println!("Applied reward signal to {} recent memories", recent);
        }
        
        Commands::Update { memory_id, new_content, reason } => {
            mem.update_memory(&memory_id, &new_content, &reason)?;
            println!("Updated memory {}", memory_id);
        }
        
        Commands::Export { path, ns } => {
            let count = mem.export_namespace(&path, ns.as_deref())?;
            println!("Exported {} memories to {}", count, path);
        }
        
        Commands::RecallAssociated { query, limit, min_confidence, ns, json } => {
            let results = mem.recall_associated_ns(
                query.as_deref(),
                limit,
                min_confidence,
                ns.as_deref(),
            )?;
            
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                if results.is_empty() {
                    println!("No associated memories found.");
                } else {
                    println!("Associated memories ({}):", results.len());
                    for r in &results {
                        println!("[{}] ({:.2}) {}", r.record.id, r.confidence, r.record.content);
                        if let Some(ref meta) = r.record.metadata {
                            if let Some(cause) = meta.get("cause_id") {
                                println!("    cause: {}", cause);
                            }
                            if let Some(effect) = meta.get("effect_id") {
                                println!("    effect: {}", effect);
                            }
                        }
                    }
                }
            }
        }
        
        Commands::Get { memory_id, json } => {
            match mem.get(&memory_id)? {
                Some(record) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&record)?);
                    } else {
                        println!("ID: {}", record.id);
                        println!("Content: {}", record.content);
                        println!("Type: {}", record.memory_type);
                        println!("Layer: {}", record.layer);
                        println!("Importance: {:.2}", record.importance);
                        println!("Pinned: {}", record.pinned);
                        println!("Working strength: {:.3}", record.working_strength);
                        println!("Core strength: {:.3}", record.core_strength);
                        println!("Created: {}", record.created_at);
                        println!("Access count: {}", record.access_times.len());
                        if !record.source.is_empty() {
                            println!("Source: {}", record.source);
                        }
                        if let Some(ref meta) = record.metadata {
                            println!("Metadata: {}", serde_json::to_string_pretty(meta)?);
                        }
                    }
                }
                None => {
                    eprintln!("Memory {} not found", memory_id);
                    std::process::exit(1);
                }
            }
        }
        
        Commands::List { limit, ns, json } => {
            let memories = mem.list_ns(ns.as_deref(), Some(limit))?;
            
            if json {
                println!("{}", serde_json::to_string_pretty(&memories)?);
            } else {
                if memories.is_empty() {
                    println!("No memories found.");
                } else {
                    println!("Memories ({}):", memories.len());
                    for m in &memories {
                        let content_preview = if m.content.len() > 60 {
                            format!("{}...", &m.content[..60])
                        } else {
                            m.content.clone()
                        };
                        println!("[{}] ({}) {}", m.id, m.memory_type, content_preview);
                    }
                }
            }
        }
        
        Commands::Bus { action } => {
            // Bus commands require workspace
            let workspace = cli.workspace.as_ref()
                .ok_or("Emotional Bus commands require --workspace")?;
            let ws_path = workspace.to_str().ok_or("invalid workspace path")?;
            
            // Create bus directly if not already attached
            let bus = EmotionalBus::new(ws_path, mem.connection())?;
            
            match action {
                BusAction::Trends { json } => {
                    let trends = bus.get_trends(mem.connection())?;
                    
                    if json {
                        println!("{}", serde_json::to_string_pretty(&trends)?);
                    } else {
                        if trends.is_empty() {
                            println!("No emotional trends recorded yet.");
                        } else {
                            println!("Emotional Trends:");
                            for trend in &trends {
                                let flag = if trend.needs_soul_update() { " ⚠️ needs update" } else { "" };
                                println!("  {}: {:.2} avg over {} events{}",
                                    trend.domain, trend.valence, trend.count, flag);
                            }
                        }
                    }
                }
                
                BusAction::Suggest { json } => {
                    let soul_updates = bus.suggest_soul_updates(mem.connection())?;
                    let heartbeat_updates = bus.suggest_heartbeat_updates(mem.connection())?;
                    
                    if json {
                        let combined = serde_json::json!({
                            "soul_updates": soul_updates,
                            "heartbeat_updates": heartbeat_updates,
                        });
                        println!("{}", serde_json::to_string_pretty(&combined)?);
                    } else {
                        if soul_updates.is_empty() && heartbeat_updates.is_empty() {
                            println!("No suggested updates at this time.");
                        } else {
                            if !soul_updates.is_empty() {
                                println!("SOUL.md Suggestions:");
                                for s in &soul_updates {
                                    println!("  [{}/{}] {}", s.domain, s.action, s.content);
                                }
                            }
                            if !heartbeat_updates.is_empty() {
                                println!("\nHEARTBEAT.md Suggestions:");
                                for h in &heartbeat_updates {
                                    println!("  [{}] {} (score: {:.0}%, {} attempts)",
                                        h.suggestion, h.action, h.stats.score * 100.0, h.stats.total);
                                }
                            }
                        }
                    }
                }
                
                BusAction::LogOutcome { action, positive, negative } => {
                    let outcome = if positive {
                        true
                    } else if negative {
                        false
                    } else {
                        return Err("Must specify --positive or --negative".into());
                    };
                    
                    bus.log_behavior(mem.connection(), &action, outcome)?;
                    let outcome_str = if outcome { "positive" } else { "negative" };
                    println!("Logged {} outcome for '{}'", outcome_str, action);
                }
                
                BusAction::BehaviorStats { json } => {
                    let stats = bus.get_behavior_stats(mem.connection())?;
                    
                    if json {
                        println!("{}", serde_json::to_string_pretty(&stats)?);
                    } else {
                        if stats.is_empty() {
                            println!("No behavior statistics recorded yet.");
                        } else {
                            println!("Behavior Statistics:");
                            for s in &stats {
                                let flag = if s.should_deprioritize() { " ⚠️ deprioritize" } else { "" };
                                println!("  {}: {:.0}% success ({}/{} positive){}",
                                    s.action, s.score * 100.0, s.positive, s.total, flag);
                            }
                        }
                    }
                }
                
                BusAction::RecordEmotion { domain, valence } => {
                    bus.process_interaction(mem.connection(), "", valence, &domain)?;
                    println!("Recorded emotion {:.2} for domain '{}'", valence, domain);
                }
                
                BusAction::Alignment { content, json } => {
                    let score = bus.alignment_score(&content);
                    let boost = bus.align_importance(&content);
                    let aligned = bus.find_aligned(&content);
                    
                    if json {
                        let result = serde_json::json!({
                            "score": score,
                            "importance_boost": boost,
                            "aligned_drives": aligned,
                        });
                        println!("{}", serde_json::to_string_pretty(&result)?);
                    } else {
                        println!("Alignment score: {:.2}", score);
                        println!("Importance boost: {:.2}x", boost);
                        if !aligned.is_empty() {
                            println!("Aligned drives:");
                            for (name, s) in &aligned {
                                println!("  {}: {:.2}", name, s);
                            }
                        }
                    }
                }
            }
        }
        
        // === Phase 3: Cross-Agent Intelligence ===
        
        Commands::Subscribe { agent_id, ns, min_importance } => {
            mem.subscribe(&agent_id, &ns, min_importance)?;
            println!("Subscribed {} to namespace '{}' (min_importance: {:.2})", 
                agent_id, ns, min_importance);
        }
        
        Commands::Unsubscribe { agent_id, ns } => {
            let removed = mem.unsubscribe(&agent_id, &ns)?;
            if removed {
                println!("Unsubscribed {} from namespace '{}'", agent_id, ns);
            } else {
                println!("No subscription found for {} on namespace '{}'", agent_id, ns);
            }
        }
        
        Commands::Notifications { agent_id, peek, json } => {
            let notifs = if peek {
                mem.peek_notifications(&agent_id)?
            } else {
                mem.check_notifications(&agent_id)?
            };
            
            if json {
                println!("{}", serde_json::to_string_pretty(&notifs)?);
            } else {
                if notifs.is_empty() {
                    println!("No pending notifications for {}", agent_id);
                } else {
                    println!("Notifications for {} ({}):", agent_id, notifs.len());
                    for n in &notifs {
                        println!("  [{}:{}] ({:.2}) {}", 
                            n.namespace, n.memory_id, n.importance, 
                            if n.content.len() > 60 {
                                format!("{}...", &n.content[..60])
                            } else {
                                n.content.clone()
                            }
                        );
                    }
                    if peek {
                        println!("\n(peeked - not marked as read)");
                    }
                }
            }
        }
        
        Commands::CrossLinks { ns_a, ns_b, json } => {
            let links = mem.discover_cross_links(&ns_a, &ns_b)?;
            
            if json {
                println!("{}", serde_json::to_string_pretty(&links)?);
            } else {
                if links.is_empty() {
                    println!("No cross-namespace links found between '{}' and '{}'", ns_a, ns_b);
                } else {
                    println!("Cross-namespace links between '{}' and '{}' ({}):", ns_a, ns_b, links.len());
                    for link in &links {
                        println!("  {} ↔ {} (strength: {:.2}, coactivations: {})",
                            link.source_id, link.target_id, link.strength, link.coactivation_count);
                    }
                }
            }
        }
        
        Commands::RecallAssoc { query, ns, limit, json } => {
            let ns_opt = if ns == "default" { None } else { Some(ns.as_str()) };
            let result = mem.recall_with_associations(&query, ns_opt, limit)?;
            
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                if result.memories.is_empty() {
                    println!("No memories found.");
                } else {
                    println!("Memories ({}):", result.memories.len());
                    for r in &result.memories {
                        println!("  [{}] ({:.2}) {}", r.record.id, r.confidence, r.record.content);
                    }
                    
                    if !result.cross_links.is_empty() {
                        println!("\nCross-namespace associations ({}):", result.cross_links.len());
                        for link in &result.cross_links {
                            let desc = link.description.as_ref()
                                .map(|d| if d.len() > 40 { format!("{}...", &d[..40]) } else { d.clone() })
                                .unwrap_or_default();
                            println!("  {}:{} → {}:{} ({:.2}) {}", 
                                link.source_ns, link.source_id, 
                                link.target_ns, link.target_id,
                                link.strength, desc);
                        }
                    }
                }
            }
        }
        
        Commands::Subscriptions { agent_id, json } => {
            let subs = mem.list_subscriptions(&agent_id)?;
            
            if json {
                println!("{}", serde_json::to_string_pretty(&subs)?);
            } else {
                if subs.is_empty() {
                    println!("No subscriptions for {}", agent_id);
                } else {
                    println!("Subscriptions for {} ({}):", agent_id, subs.len());
                    for sub in &subs {
                        println!("  {} (min_importance: {:.2}, since: {})",
                            sub.namespace, sub.min_importance, sub.created_at.format("%Y-%m-%d %H:%M"));
                    }
                }
            }
        }
        
        // === Embedding Commands ===
        
        Commands::Reindex { progress } => {
            if !mem.has_embedding_support() {
                eprintln!("Error: Ollama not available. Cannot reindex embeddings.");
                eprintln!("Make sure Ollama is running at {}", mem.embedding_config().host);
                std::process::exit(1);
            }
            
            if progress {
                let count = mem.reindex_embeddings_with_progress(|current, total| {
                    eprint!("\rReindexing: {}/{}", current, total);
                })?;
                eprintln!();
                println!("Reindexed {} memories", count);
            } else {
                let count = mem.reindex_embeddings()?;
                println!("Reindexed {} memories", count);
            }
        }
        
        Commands::EmbeddingStatus { json } => {
            let stats = mem.embedding_stats()?;
            let config = mem.embedding_config();
            let enabled = mem.has_embedding_support();
            let available = mem.is_embedding_available();
            
            if json {
                let result = serde_json::json!({
                    "enabled": enabled,
                    "available": available,
                    "provider": config.provider,
                    "model": config.model,
                    "host": config.host,
                    "dimensions": config.dimensions,
                    "total_memories": stats.total_memories,
                    "embedded_count": stats.embedded_count,
                    "pending_count": stats.total_memories - stats.embedded_count,
                });
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("Embedding Status:");
                println!("  Provider: {}", config.provider);
                println!("  Model: {}", config.model);
                println!("  Host: {}", config.host);
                println!("  Dimensions: {}", config.dimensions);
                println!("  Enabled: {}", if enabled { "yes" } else { "no (Ollama not found at startup)" });
                println!("  Ollama available now: {}", if available { "yes" } else { "no" });
                println!();
                println!("Memory Coverage:");
                println!("  Total memories: {}", stats.total_memories);
                println!("  With embeddings: {}", stats.embedded_count);
                println!("  Pending: {}", stats.total_memories - stats.embedded_count);
                
                if stats.total_memories > 0 {
                    let coverage = stats.embedded_count as f64 / stats.total_memories as f64 * 100.0;
                    println!("  Coverage: {:.1}%", coverage);
                }
                
                if !enabled {
                    println!();
                    println!("Note: Embedding is disabled because Ollama was not available when the");
                    println!("      memory system was initialized. Start Ollama and restart to enable.");
                }
            }
        }
        
        // === Entity Management ===
        
        Commands::Entities { command, entity_type } => {
            match command {
                Some(EntityCommand::Backfill { batch_size }) => {
                    println!("⏳ Backfilling entities from existing memories...");
                    let (processed, entities, relations) = mem.backfill_entities(batch_size)?;
                    println!("✅ Processed: {} memories, {} entities, {} relations", 
                        processed, entities, relations);
                }
                Some(EntityCommand::Purge) => {
                    println!("🧹 Purging garbage entities...");
                    let deleted = mem.purge_garbage_entities()?;
                    println!("✅ Deleted {} garbage entities", deleted);
                    let (entity_count, relation_count, link_count) = mem.entity_stats()?;
                    println!("📊 Remaining: {} entities, {} relations, {} links", 
                        entity_count, relation_count, link_count);
                }
                Some(EntityCommand::Stats) => {
                    let (entity_count, relation_count, link_count) = mem.entity_stats()?;
                    println!("📊 Entity Index:");
                    println!("  Entities:  {:>5}", entity_count);
                    println!("  Relations: {:>5}", relation_count);
                    println!("  Links:     {:>5}", link_count);
                }
                Some(EntityCommand::List { entity_type: list_type, limit }) => {
                    let filter_type = list_type.as_deref();
                    let entities = mem.list_entities(filter_type, None, limit)?;
                    
                    if entities.is_empty() {
                        println!("No entities found.");
                    } else {
                        let type_label = filter_type.map(|t| format!(" [{}]", t)).unwrap_or_default();
                        println!("📊 Entities{} (top {}):", type_label, entities.len());
                        for (entity, mentions) in &entities {
                            println!("  {:<20} [{:<12}] {:>3} mentions", entity.name, entity.entity_type, mentions);
                        }
                    }
                }
                None => {
                    // Default: list top 20 entities by mention count, filtered by --type if given
                    let filter_type = entity_type.as_deref();
                    let entities = mem.list_entities(filter_type, None, 20)?;
                    
                    if entities.is_empty() {
                        println!("No entities found.");
                    } else {
                        let type_label = filter_type.map(|t| format!(" [{}]", t)).unwrap_or_default();
                        println!("📊 Entities{} (top {}):", type_label, entities.len());
                        for (entity, mentions) in &entities {
                            println!("  {:<20} [{:<12}] {:>3} mentions", entity.name, entity.entity_type, mentions);
                        }
                    }
                }
            }
        }

        Commands::Synthesize { dry_run, json } => {
            // Enable synthesis with defaults for this run
            let mut settings = engramai::SynthesisSettings::default();
            settings.enabled = true;

            if dry_run {
                mem.set_synthesis_settings(settings);
                let report = mem.synthesize_dry_run()?;

                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!("Cluster Discovery: found {} clusters", report.clusters_found);
                    println!();
                    let mut synth_count = 0;
                    let mut skip_count = 0;
                    let mut defer_count = 0;
                    let mut auto_count = 0;
                    for gr in &report.gate_results {
                        match &gr.decision {
                            engramai::GateDecision::Synthesize { .. } => synth_count += 1,
                            engramai::GateDecision::Skip { .. } => skip_count += 1,
                            engramai::GateDecision::Defer { .. } => defer_count += 1,
                            engramai::GateDecision::AutoUpdate { .. } => auto_count += 1,
                        }
                    }
                    println!("Gate Check:");
                    println!("  SYNTHESIZE:  {} clusters (ready for LLM)", synth_count);
                    println!("  AUTO_UPDATE: {} clusters (existing insight covers)", auto_count);
                    println!("  SKIP:        {} clusters (near-duplicate/covered)", skip_count);
                    println!("  DEFER:       {} clusters (too recent/small)", defer_count);
                    println!();
                    println!("Dry run — no changes made. Run without --dry-run to execute.");
                }
            } else {
                mem.set_synthesis_settings(settings);
                let report = mem.synthesize()?;

                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!("Synthesis Cycle");
                    println!("  Duration:     {:.1}s", report.duration.as_secs_f64());
                    println!("  Clusters:     {} discovered, {} synthesized, {} auto-updated, {} skipped, {} deferred",
                        report.clusters_found, report.clusters_synthesized,
                        report.clusters_auto_updated, report.clusters_skipped, report.clusters_deferred);
                    println!("  Insights:     {} created", report.insights_created.len());
                    println!("  Demotions:    {} sources demoted", report.sources_demoted.len());

                    if !report.insights_created.is_empty() {
                        println!();
                        for (i, id) in report.insights_created.iter().enumerate() {
                            if let Ok(Some(record)) = mem.get(id) {
                                let preview: String = record.content.chars().take(80).collect();
                                println!("  [{}] {} — \"{}\"", i + 1, id, preview);
                            }
                        }
                    }

                    if !report.errors.is_empty() {
                        println!();
                        println!("  Errors:");
                        for e in &report.errors {
                            println!("    ⚠️  {}", e);
                        }
                    }
                }
            }
        }

        Commands::Insights { limit, json } => {
            let insights = mem.list_insights(Some(limit))?;

            if json {
                println!("{}", serde_json::to_string_pretty(&insights)?);
            } else {
                if insights.is_empty() {
                    println!("No insights found. Run `engram synthesize` first.");
                } else {
                    println!("{:<12} {:<8} {:<8} {:<12} {}", "ID", "Type", "Sources", "Created", "Content");
                    for insight in &insights {
                        let meta = insight.metadata.as_ref();
                        let synth_type = meta
                            .and_then(|m| m.get("insight_type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let source_count = meta
                            .and_then(|m| m.get("source_count"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let created = insight.created_at.format("%Y-%m-%d");
                        let preview: String = insight.content.chars().take(60).collect();
                        let short_id: String = insight.id.chars().take(10).collect();
                        println!("{:<12} {:<8} {:<8} {:<12} {}",
                            short_id, synth_type, source_count, created, preview);
                    }
                    println!("\nTotal: {} insights", insights.len());
                }
            }
        }

        Commands::Insight { id, sources, provenance, json } => {
            // Show insight details
            let record = mem.get(&id)?;
            match record {
                None => {
                    eprintln!("Error: memory '{}' not found", id);
                    std::process::exit(1);
                }
                Some(record) => {
                    if json {
                        let mut output = serde_json::to_value(&record)?;
                        if sources {
                            let src = mem.insight_sources(&id)?;
                            output["sources"] = serde_json::to_value(&src)?;
                        }
                        if provenance {
                            let chain = mem.get_provenance(&id, 5)?;
                            output["provenance"] = serde_json::to_value(&chain)?;
                        }
                        println!("{}", serde_json::to_string_pretty(&output)?);
                    } else {
                        println!("Insight: {}", record.id);
                        let meta = record.metadata.as_ref();
                        let synth_type = meta
                            .and_then(|m| m.get("insight_type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let confidence = meta
                            .and_then(|m| m.get("confidence"))
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        println!("Type:    {}", synth_type);
                        println!("Conf:    {:.2}", confidence);
                        println!("Created: {}", record.created_at.format("%Y-%m-%dT%H:%M:%SZ"));
                        println!("Content: {}", record.content);

                        if sources {
                            let src_records = mem.insight_sources(&id)?;
                            if src_records.is_empty() {
                                println!("\nNo provenance records found.");
                            } else {
                                println!("\nSources ({}):", src_records.len());
                                println!("  {:<12} {:<10} {:<10} {}", "ID", "Orig Imp.", "Confidence", "Content");
                                for pr in &src_records {
                                    if let Ok(Some(src_mem)) = mem.get(&pr.source_id) {
                                        let preview: String = src_mem.content.chars().take(50).collect();
                                        let short_id: String = pr.source_id.chars().take(10).collect();
                                        let orig_imp = pr.source_original_importance.unwrap_or(0.0);
                                        println!("  {:<12} {:<10.2} {:<10.2} {}",
                                            short_id, orig_imp, pr.confidence, preview);
                                    }
                                }
                            }
                        }

                        if provenance {
                            let chain = mem.get_provenance(&id, 5)?;
                            let total: usize = chain.layers.iter().map(|l| l.len()).sum();
                            println!("\nProvenance Chain ({} records across {} layers):", total, chain.layers.len());
                            for (depth, layer) in chain.layers.iter().enumerate() {
                                for pr in layer {
                                    let indent = "  ".repeat(depth + 1);
                                    println!("{}← {} (confidence: {:.2})", indent, pr.source_id, pr.confidence);
                                }
                            }
                        }
                    }
                }
            }
        }

        Commands::Sleep { ns, days, json } => {
            // Enable synthesis for the sleep cycle
            let mut settings = engramai::SynthesisSettings::default();
            settings.enabled = true;
            mem.set_synthesis_settings(settings);

            let report = mem.sleep_cycle(days, ns.as_deref())?;

            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "consolidation_ok": report.consolidation_ok,
                    "synthesis": report.synthesis,
                }))?);
            } else {
                println!("Sleep Cycle Complete");
                println!("  Consolidation: ✅ ({:.1} days)", days);
                match report.synthesis {
                    Some(synth) => {
                        println!("  Synthesis:     ✅ ({} clusters, {} insights, {:.1}s)",
                            synth.clusters_found, synth.insights_created.len(), synth.duration.as_secs_f64());
                    }
                    None => {
                        println!("  Synthesis:     ⏭️  (not enabled or no clusters)");
                    }
                }
            }
        }

        Commands::Unsynthesise { id } => {
            let result = mem.reverse_synthesis(&id)?;
            println!("✅ Reversed synthesis of insight {}", result.insight_id);
            println!("  Sources restored: {}", result.restored_sources.len());
            for src in &result.restored_sources {
                println!("    {} — original importance: {:.2}, restored: {}", 
                    src.memory_id, src.original_importance, if src.restored { "✅" } else { "❌" });
            }
        }

        Commands::Knowledge(cmd) => {
            // Open knowledge store backed by the same database
            let kc_store = SqliteKnowledgeStore::open(&cli.database)
                .map_err(|e| format!("Failed to open knowledge store: {}", e))?;
            kc_store.init_schema()
                .map_err(|e| format!("Failed to init KC schema: {}", e))?;
            let kc_config = KcConfig::load();

            match cmd {
                KnowledgeCommand::Query { search, limit, format } => {
                    let topics = kc_store.list_topic_pages()
                        .map_err(|e| format!("Failed to list topics: {}", e))?;
                    let search_lower = search.to_lowercase();
                    let matched: Vec<_> = topics.iter()
                        .filter(|t| {
                            t.title.to_lowercase().contains(&search_lower)
                                || t.content.to_lowercase().contains(&search_lower)
                                || t.summary.to_lowercase().contains(&search_lower)
                        })
                        .take(limit)
                        .collect();

                    match format {
                        OutputFormat::Json => {
                            println!("{}", serde_json::to_string_pretty(&matched)?);
                        }
                        _ => {
                            if matched.is_empty() {
                                println!("No topics matching '{}'.", search);
                            } else {
                                println!("Knowledge topics matching '{}' ({}):", search, matched.len());
                                for t in &matched {
                                    println!("  [{}] {} (v{}, {:?})", t.id, t.title, t.version, t.status);
                                    if !t.summary.is_empty() {
                                        let preview: String = t.summary.chars().take(80).collect();
                                        println!("    {}", preview);
                                    }
                                }
                            }
                        }
                    }
                }

                KnowledgeCommand::Inspect { topic_id, sources, conflicts } => {
                    let tid = TopicId(topic_id.clone());
                    match kc_store.get_topic_page(&tid)
                        .map_err(|e| format!("Failed to get topic: {}", e))? {
                        Some(page) => {
                            println!("Topic: {} ({})", page.title, page.id);
                            println!("Status:  {:?}", page.status);
                            println!("Version: {}", page.version);
                            println!("Created: {}", page.metadata.created_at.format("%Y-%m-%dT%H:%M:%SZ"));
                            println!("Updated: {}", page.metadata.updated_at.format("%Y-%m-%dT%H:%M:%SZ"));
                            if let Some(q) = page.metadata.quality_score {
                                println!("Quality: {:.2}", q);
                            }
                            if !page.metadata.tags.is_empty() {
                                println!("Tags:    {}", page.metadata.tags.join(", "));
                            }
                            println!("\nSummary: {}", page.summary);
                            println!("\n{}", page.content);

                            if sources {
                                let refs = kc_store.get_source_refs(&tid)
                                    .map_err(|e| format!("Failed to get source refs: {}", e))?;
                                if refs.is_empty() {
                                    println!("\nNo source memories.");
                                } else {
                                    println!("\nSource memories ({}):", refs.len());
                                    for r in &refs {
                                        println!("  {} (relevance: {:.3}, added: {})",
                                            r.memory_id, r.relevance_score,
                                            r.added_at.format("%Y-%m-%dT%H:%M:%SZ"));
                                    }
                                }
                            }

                            if conflicts {
                                let all_topics = kc_store.list_topic_pages()
                                    .map_err(|e| format!("Failed to list topics: {}", e))?;
                                let detector = ConflictDetector::new();
                                let scope = ConflictScope::WithinTopic(tid.clone());
                                match detector.detect_conflicts(&all_topics, &scope, None) {
                                    Ok(records) => {
                                        if records.is_empty() {
                                            println!("\nNo conflicts detected.");
                                        } else {
                                            println!("\nConflicts ({}):", records.len());
                                            for cr in &records {
                                                println!("  [{:?}] {} ({:?})",
                                                    cr.severity, cr.conflict.description, cr.conflict.conflict_type);
                                            }
                                        }
                                    }
                                    Err(e) => println!("\nConflict detection failed: {}", e),
                                }
                            }
                        }
                        None => {
                            eprintln!("Topic '{}' not found.", topic_id);
                            std::process::exit(1);
                        }
                    }
                }

                KnowledgeCommand::Export { output, format } => {
                    let privacy = PrivacyGuard::in_memory()
                        .map_err(|e| format!("Failed to create privacy guard: {}", e))?;
                    let ctx = AccessContext {
                        accessor: cli.agent_id.clone().unwrap_or_else(|| "cli".to_string()),
                        include_private: false,
                        is_export: true,
                    };
                    let filter = ExportFilter {
                        topics: None,
                        status: None,
                        tags: None,
                        since: None,
                    };
                    let export_fmt = match format {
                        OutputFormat::Json => compiler::types::ExportFormat::Json,
                        OutputFormat::Md | OutputFormat::Text => compiler::types::ExportFormat::Markdown,
                    };
                    let result = ExportEngine::export(&kc_store, &privacy, &ctx, &filter, export_fmt)
                        .map_err(|e| format!("Export failed: {}", e))?;

                    match result {
                        ExportOutput::Json(json_str) => {
                            let path = format!("{}.json", output);
                            std::fs::write(&path, &json_str)?;
                            println!("Exported knowledge to {}", path);
                        }
                        ExportOutput::Markdown(files) => {
                            std::fs::create_dir_all(&output)?;
                            for file in &files {
                                let path = std::path::Path::new(&output).join(&file.path);
                                std::fs::write(&path, &file.content)?;
                            }
                            println!("Exported {} topic(s) to {}/", files.len(), output);
                        }
                    }
                }

                KnowledgeCommand::Import { path } => {
                    let import_path = std::path::Path::new(&path);
                    if !import_path.exists() {
                        eprintln!("Path does not exist: {}", path);
                        std::process::exit(1);
                    }
                    let importer = MarkdownImporter {
                        split: kc_config.import.split_strategy.clone(),
                    };
                    let report = ImportPipeline::run(&kc_store, &importer, import_path, &kc_config.import)
                        .map_err(|e| format!("Import failed: {}", e))?;
                    println!("Import complete:");
                    println!("  Processed: {}", report.total_processed);
                    println!("  Imported:  {}", report.imported);
                    println!("  Skipped:   {}", report.skipped);
                    if !report.errors.is_empty() {
                        println!("  Errors:    {}", report.errors.len());
                        for err in &report.errors {
                            println!("    - {}", err);
                        }
                    }
                }

                KnowledgeCommand::Health { scope: _, format, json } => {
                    let decay_engine = DecayEngine::new(kc_config.decay.clone());
                    let conflict_detector = ConflictDetector::new();
                    let auditor = HealthAuditor;

                    match auditor.health_report(&kc_store, &decay_engine, &conflict_detector) {
                        Ok(report) => {
                            if json || matches!(format, OutputFormat::Json) {
                                println!("{}", serde_json::to_string_pretty(&report)?);
                            } else {
                                println!("Knowledge Health Report");
                                println!("  Total topics:     {}", report.total_topics);
                                println!("  Stale topics:     {}", report.stale_topics.len());
                                println!("  Broken links:     {}", report.broken_links.len());
                                println!("  Recommendations:  {}", report.recommendations.len());
                                for rec in &report.recommendations {
                                    println!("    [P{}] {} — {} ({})",
                                        rec.priority, rec.topic_id, rec.action, rec.reason);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Health report failed: {}", e);
                            std::process::exit(1);
                        }
                    }
                }

                KnowledgeCommand::Decay { evaluate: _, apply, topic } => {
                    let decay_engine = DecayEngine::new(kc_config.decay.clone());

                    if let Some(ref topic_id) = topic {
                        let tid = TopicId(topic_id.clone());
                        match kc_store.get_topic_page(&tid)
                            .map_err(|e| format!("Failed to get topic: {}", e))? {
                            Some(page) => {
                                let result = decay_engine.evaluate_topic(&page, &kc_store)
                                    .map_err(|e| format!("Decay evaluation failed: {}", e))?;
                                println!("Decay evaluation for '{}':", page.title);
                                println!("  Freshness:    {:.3}", result.freshness_score);
                                println!("  Sources:      {}", result.source_count);
                                println!("  Action:       {:?}", result.recommended_action);
                                if apply {
                                    decay_engine.apply_decay(&result.recommended_action, &kc_store)
                                        .map_err(|e| format!("Apply decay failed: {}", e))?;
                                    println!("  ✅ Applied.");
                                }
                            }
                            None => {
                                eprintln!("Topic '{}' not found.", topic_id);
                                std::process::exit(1);
                            }
                        }
                    } else {
                        let results = decay_engine.evaluate_all(&kc_store)
                            .map_err(|e| format!("Decay evaluation failed: {}", e))?;
                        if results.is_empty() {
                            println!("No active topics to evaluate.");
                        } else {
                            println!("Decay evaluation ({} topics):", results.len());
                            for r in &results {
                                println!("  [{}] freshness={:.3} sources={} action={:?}",
                                    r.topic_id, r.freshness_score, r.source_count, r.recommended_action);
                            }
                            if apply {
                                for r in &results {
                                    decay_engine.apply_decay(&r.recommended_action, &kc_store)
                                        .map_err(|e| format!("Apply decay failed: {}", e))?;
                                }
                                println!("✅ Applied decay to {} topics.", results.len());
                            }
                        }
                    }
                }

                KnowledgeCommand::Conflicts { scan: _, json } => {
                    let all_topics = kc_store.list_topic_pages()
                        .map_err(|e| format!("Failed to list topics: {}", e))?;
                    if all_topics.len() < 2 {
                        println!("Need at least 2 topics to detect conflicts.");
                        return Ok(());
                    }
                    let detector = ConflictDetector::new();
                    let mut all_conflicts = Vec::new();
                    for topic in &all_topics {
                        let scope = ConflictScope::WithinTopic(topic.id.clone());
                        match detector.detect_conflicts(&all_topics, &scope, None) {
                            Ok(mut conflicts) => all_conflicts.append(&mut conflicts),
                            Err(e) => eprintln!("Warning: conflict detection failed for {}: {}", topic.id, e),
                        }
                    }
                    // Deduplicate by conflict id
                    all_conflicts.sort_by(|a, b| a.conflict.id.0.cmp(&b.conflict.id.0));
                    all_conflicts.dedup_by(|a, b| a.conflict.id.0 == b.conflict.id.0);

                    if json {
                        println!("{}", serde_json::to_string_pretty(&all_conflicts)?);
                    } else {
                        if all_conflicts.is_empty() {
                            println!("No conflicts detected.");
                        } else {
                            println!("Conflicts ({}):", all_conflicts.len());
                            for cr in &all_conflicts {
                                println!("  [{:?}] {} ({:?})",
                                    cr.severity, cr.conflict.description, cr.conflict.conflict_type);
                            }
                        }
                    }
                }

                KnowledgeCommand::Audit { links, duplicates, repair } => {
                    let auditor = HealthAuditor;
                    let all_topics = kc_store.list_topic_pages()
                        .map_err(|e| format!("Failed to list topics: {}", e))?;

                    if links || (!links && !duplicates) {
                        println!("Link Audit:");
                        let mut total_broken = 0usize;
                        let mut total_stale = 0usize;
                        for topic in &all_topics {
                            let entries = auditor.audit_links(topic, &kc_store)
                                .map_err(|e| format!("Audit failed for {}: {}", topic.id, e))?;
                            let broken: Vec<_> = entries.iter()
                                .filter(|e| e.status == LinkStatus::Broken)
                                .collect();
                            let stale: Vec<_> = entries.iter()
                                .filter(|e| e.status == LinkStatus::Stale)
                                .collect();
                            if !broken.is_empty() || !stale.is_empty() {
                                println!("  [{}] {} broken, {} stale links",
                                    topic.id, broken.len(), stale.len());
                            }
                            total_broken += broken.len();
                            total_stale += stale.len();
                        }
                        println!("  Total: {} broken, {} stale links across {} topics",
                            total_broken, total_stale, all_topics.len());
                    }

                    if duplicates {
                        let detector = ConflictDetector::new();
                        let dup_groups = detector.detect_duplicates(&all_topics);
                        if dup_groups.is_empty() {
                            println!("\nNo duplicate groups found.");
                        } else {
                            println!("\nDuplicate groups ({}):", dup_groups.len());
                            for group in &dup_groups {
                                println!("  Canonical: {} (similarity: {:.1}%)",
                                    group.canonical, group.similarity * 100.0);
                                for dup in &group.duplicates {
                                    println!("    duplicate: {}", dup);
                                }
                            }
                        }
                    }

                    if repair {
                        println!("\n⚠️  Auto-repair is not yet implemented. Use 'engram knowledge health' for recommendations.");
                    }
                }

                KnowledgeCommand::Privacy { set_level, level, audit_log } => {
                    let privacy = PrivacyGuard::in_memory()
                        .map_err(|e| format!("Failed to create privacy guard: {}", e))?;

                    if let Some(ref topic_id) = set_level {
                        let tid = TopicId(topic_id.clone());
                        match level.as_deref() {
                            Some(lvl) => {
                                // Privacy level is tag-based; update topic tags
                                match kc_store.get_topic_page(&tid)
                                    .map_err(|e| format!("Failed to get topic: {}", e))? {
                                    Some(mut page) => {
                                        // Remove existing privacy tags
                                        page.metadata.tags.retain(|t| !t.starts_with("privacy:"));
                                        // Add new privacy tag
                                        let tag = match lvl {
                                            "private" => "privacy:private",
                                            "sensitive" => "privacy:sensitive",
                                            "internal" => "privacy:internal",
                                            "public" => "privacy:public",
                                            _ => {
                                                eprintln!("Invalid privacy level: {}. Use: public, internal, sensitive, private", lvl);
                                                std::process::exit(1);
                                            }
                                        };
                                        if tag != "privacy:public" {
                                            page.metadata.tags.push(tag.to_string());
                                        }
                                        kc_store.update_topic_page(&page)
                                            .map_err(|e| format!("Failed to update topic: {}", e))?;
                                        println!("Set privacy level for '{}' to '{}'.", topic_id, lvl);
                                    }
                                    None => {
                                        eprintln!("Topic '{}' not found.", topic_id);
                                        std::process::exit(1);
                                    }
                                }
                            }
                            None => {
                                eprintln!("--level is required when using --set-level. Use: public, internal, sensitive, private");
                                std::process::exit(1);
                            }
                        }
                    } else if audit_log {
                        let entries = privacy.query_audit_log(None, 50)
                            .map_err(|e| format!("Failed to query audit log: {}", e))?;
                        if entries.is_empty() {
                            println!("No audit log entries.");
                        } else {
                            println!("Audit log ({} entries):", entries.len());
                            for entry in &entries {
                                let tid = entry.topic_id.as_ref()
                                    .map(|t| t.0.as_str())
                                    .unwrap_or("-");
                                println!("  {} [{}] topic={} actor={} {}",
                                    entry.timestamp.format("%Y-%m-%dT%H:%M:%SZ"),
                                    entry.operation, tid, entry.actor, entry.details);
                            }
                        }
                    } else {
                        println!("Usage:");
                        println!("  engram knowledge privacy --set-level <TOPIC_ID> --level <LEVEL>");
                        println!("  engram knowledge privacy --audit-log");
                    }
                }

                KnowledgeCommand::Compile { topic, dry_run } => {
                    // Step 1: Read all memories
                    let all_memories = mem.list_ns(None, None)
                        .map_err(|e| format!("Failed to read memories: {}", e))?;

                    if all_memories.is_empty() {
                        println!("No memories found. Add some memories first with `engram store`.");
                        return Ok(());
                    }

                    println!("📚 Found {} memories to analyze", all_memories.len());

                    // Step 2: Load real embeddings from memory_embeddings table
                    let engram_storage = engramai::storage::Storage::new(db_path)?;
                    let emb_config = engramai::embeddings::EmbeddingConfig::default();
                    let model_id = emb_config.model_id(); // "ollama/nomic-embed-text"
                    let embedding_map: std::collections::HashMap<String, Vec<f32>> = engram_storage
                        .get_all_embeddings(&model_id)
                        .unwrap_or_default()
                        .into_iter()
                        .collect();

                    // Step 3: Convert MemoryRecord → MemorySnapshot
                    let snapshots: Vec<MemorySnapshot> = all_memories.iter().map(|m| {
                        let tags = m.metadata.as_ref()
                            .and_then(|v| v.get("tags"))
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                            .unwrap_or_default();
                        MemorySnapshot {
                            id: m.id.clone(),
                            content: m.content.clone(),
                            memory_type: format!("{:?}", m.memory_type).to_lowercase(),
                            importance: m.importance,
                            created_at: m.created_at,
                            updated_at: m.created_at,
                            tags,
                            embedding: embedding_map.get(&m.id).cloned(),
                        }
                    }).collect();

                    match topic {
                        Some(ref topic_id) => {
                            // Recompile a specific topic
                            let tid = TopicId(topic_id.clone());
                            let existing = kc_store.get_topic_page(&tid)
                                .map_err(|e| format!("Failed to get topic: {}", e))?;
                            match existing {
                                Some(page) => {
                                    if dry_run {
                                        println!("🔍 Dry run: would recompile topic '{}'", page.title);
                                        println!("   Current version: {}", page.version);
                                        println!("   Source memories: {}", page.metadata.source_memory_ids.len());
                                        println!("   Available memories for matching: {}", snapshots.len());
                                    } else {
                                        println!("🔄 Recompiling topic '{}'...", page.title);
                                        // Filter snapshots to those relevant to this topic
                                        let relevant: Vec<MemorySnapshot> = snapshots.iter()
                                            .filter(|s| page.metadata.source_memory_ids.contains(&s.id))
                                            .cloned()
                                            .collect();

                                        let api = MaintenanceApi::new(
                                            SqliteKnowledgeStore::open(&cli.database)
                                                .map_err(|e| format!("Failed to open store: {}", e))?,
                                            kc_config.clone(),
                                        );

                                        let pages = api.compile_all::<NoopProvider>(None, &relevant)
                                            .map_err(|e| format!("Compilation failed: {}", e))?;

                                        println!("✅ Recompiled {} topic(s)", pages.len());
                                        for p in &pages {
                                            let q = p.metadata.quality_score.map(|s| format!(" q={:.2}", s)).unwrap_or_default();
                                            println!("   [{}] {} (v{}{})", p.id.0, p.title, p.version, q);
                                        }
                                    }
                                }
                                None => {
                                    eprintln!("Topic not found: {}", topic_id);
                                    std::process::exit(1);
                                }
                            }
                        }
                        None => {
                            // Auto-discovery mode
                            if dry_run {
                                // Run discovery only, show what would be compiled
                                let embeddings: Vec<(String, Vec<f32>)> = snapshots.iter()
                                    .map(|m| {
                                        let emb = m.embedding.clone()
                                            .unwrap_or_else(|| {
                                                engramai::compiler::compilation::simple_hash_embedding(&m.content, 64)
                                            });
                                        (m.id.clone(), emb)
                                    })
                                    .collect();

                                let discovery = TopicDiscovery::new(kc_config.min_cluster_size);
                                let candidates = discovery.discover(&embeddings);

                                if candidates.is_empty() {
                                    println!("🔍 No topic candidates discovered.");
                                    println!("   Try adding more related memories (min cluster size: {})", kc_config.min_cluster_size);
                                } else {
                                    println!("🔍 Dry run: discovered {} topic candidate(s):", candidates.len());
                                    for (i, c) in candidates.iter().enumerate() {
                                        let title = c.suggested_title.as_deref().unwrap_or("(untitled)");
                                        println!("   {}. {} ({} memories)", i + 1, title, c.memories.len());
                                        for mid in &c.memories {
                                            if let Some(snap) = snapshots.iter().find(|s| &s.id == mid) {
                                                let preview: String = snap.content.chars().take(80).collect();
                                                println!("      - [{}] {}", mid, preview);
                                            }
                                        }
                                    }
                                }
                            } else {
                                // Full compilation
                                println!("🔬 Discovering topic clusters...");

                                let api = MaintenanceApi::new(
                                    SqliteKnowledgeStore::open(&cli.database)
                                        .map_err(|e| format!("Failed to open store: {}", e))?,
                                    kc_config.clone(),
                                );

                                let pages = api.compile_all::<NoopProvider>(None, &snapshots)
                                    .map_err(|e| format!("Compilation failed: {}", e))?;

                                if pages.is_empty() {
                                    println!("No topic candidates discovered from {} memories.", snapshots.len());
                                    println!("   Memories may not form clear clusters yet.");
                                    println!("   Minimum cluster size: {}", kc_config.min_cluster_size);
                                } else {
                                    println!("✅ Compiled {} topic(s):", pages.len());
                                    for p in &pages {
                                        let q = p.metadata.quality_score.map(|s| format!(" q={:.2}", s)).unwrap_or_default();
                                        println!("   [{}] {} ({} sources, v{}{})",
                                            p.id.0, p.title, p.metadata.source_memory_ids.len(), p.version, q);
                                    }
                                    println!("\nUse `engram knowledge list` to see all topics.");
                                    println!("Use `engram knowledge inspect <topic-id>` for details.");
                                }
                            }
                        }
                    }
                }

                KnowledgeCommand::List { json, limit } => {
                    let topics = kc_store.list_topic_pages()
                        .map_err(|e| format!("Failed to list topics: {}", e))?;
                    let topics: Vec<_> = topics.into_iter().take(limit).collect();

                    if json {
                        println!("{}", serde_json::to_string_pretty(&topics)?);
                    } else {
                        if topics.is_empty() {
                            println!("No compiled knowledge topics.");
                        } else {
                            println!("Knowledge topics ({}):", topics.len());
                            for t in &topics {
                                let quality = t.metadata.quality_score
                                    .map(|q| format!(" q={:.2}", q))
                                    .unwrap_or_default();
                                println!("  [{}] {} (v{}, {:?}{})",
                                    t.id, t.title, t.version, t.status, quality);
                            }
                        }
                    }
                }
            }
        }
    }
    
    Ok(())
}
