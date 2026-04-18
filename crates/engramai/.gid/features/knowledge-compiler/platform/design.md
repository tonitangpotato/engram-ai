# Design: Platform Setup, LLM Config, Import & Intake

> Feature-level design for the platform infrastructure layer.
> Requirements: `.gid/features/knowledge-compiler/platform/requirements.md`
> Architecture: `.gid/docs/architecture.md`

## §1 Overview

This feature provides the infrastructure foundation that compilation and maintenance depend on:
- Multi-provider LLM abstraction (OpenAI, Anthropic, local models)
- Zero-config setup with progressive disclosure of options
- Embedding model auto-download and management
- Import from external knowledge sources (Logseq, Obsidian, markdown)
- Intake pipeline for URL-based content extraction

All platform components are consumed by other features but never consume them — platform is pure infrastructure with no knowledge of topic semantics.

### Requirements Coverage

| Component | GOALs |
|-----------|-------|
| §2.1 LLM Provider | GOAL-plat.1, GOAL-plat.2 |
| §2.2 Configuration | GOAL-plat.2, GOAL-plat.16 |
| §2.3 Embedding Pipeline | GOAL-plat.4, GOAL-plat.5 |
| §2.4 Import/Export | GOAL-plat.8, GOAL-plat.9, GOAL-plat.10, GOAL-plat.11, GOAL-plat.15 |
| §2.5 Intake Pipeline | GOAL-plat.12, GOAL-plat.13, GOAL-plat.14 |
| §2.6 Graceful Degradation | GOAL-plat.3, GOAL-plat.5 (fallback) |
| §2.7 Installation & Setup | GOAL-plat.6 |
| §2.8 Feature Flags | GOAL-plat.7 |

---

## §2 Components

### §2.1 LLM Provider Abstraction

**Satisfies:** GOAL-plat.1 (multi-provider), GOAL-plat.2 (LLM configuration file)

#### Core Trait

```rust
/// Provider-agnostic LLM interface.
/// All compilation/maintenance code programs against this trait.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Generate a completion from a prompt + optional system message.
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;

    /// Return provider metadata (name, model, capabilities).
    fn metadata(&self) -> ProviderMetadata;

    /// Check if provider is available (API key set, endpoint reachable).
    async fn health_check(&self) -> Result<(), LlmError>;
}

pub struct LlmRequest {
    pub system: Option<String>,
    pub prompt: String,
    pub max_tokens: u32,
    pub temperature: f32,
    /// Caller-specified token budget — provider must refuse if prompt exceeds
    pub token_budget: Option<u32>,
}

pub struct LlmResponse {
    pub content: String,
    pub usage: TokenUsage,
    pub model: String,
}

pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

pub struct ProviderMetadata {
    pub name: &'static str,        // "openai", "anthropic", "local"
    pub model: String,              // "gpt-4o", "claude-sonnet-4-20250514", "llama3"
    pub max_context: u32,           // token limit
    pub supports_streaming: bool,
}
```

#### Implementations

```rust
pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    endpoint: String,  // default: api.openai.com, overridable for Azure/compatible
}

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

pub struct LocalProvider {
    /// Endpoint for local model (llama.cpp, Ollama)
    endpoint: String,
    model: String,
}
```

#### Task-Based Model Selection (GOAL-plat.2)

Different KC tasks have different quality/cost needs:

```rust
pub enum LlmTask {
    /// Topic naming — needs creativity, short output
    TopicNaming,
    /// Content synthesis — needs coherence, medium output
    TopicRendering,
    /// Conflict detection — needs precision, short output
    ConflictAnalysis,
    /// Enhancement — needs quality, long output
    ContentEnhancement,
}

pub struct ModelRouter {
    /// Task → model override mapping from config
    task_models: HashMap<LlmTask, String>,
    /// Default provider
    default: Box<dyn LlmProvider>,
    /// Named providers
    providers: HashMap<String, Box<dyn LlmProvider>>,
}

impl ModelRouter {
    /// Select provider+model for a given task.
    /// Falls back to default if no task-specific mapping.
    pub fn for_task(&self, task: LlmTask) -> &dyn LlmProvider {
        self.task_models
            .get(&task)
            .and_then(|name| self.providers.get(name))
            .map(|p| p.as_ref())
            .unwrap_or(self.default.as_ref())
    }
}
```

#### Token Budget Enforcement (GOAL-plat.3)

```rust
impl OpenAiProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        // Pre-flight check: estimate prompt tokens
        let estimated = estimate_tokens(&request.prompt, &self.model);
        if let Some(budget) = request.token_budget {
            if estimated > budget {
                return Err(LlmError::BudgetExceeded {
                    estimated,
                    budget,
                });
            }
        }
        // ... actual API call
    }
}
```

Token estimation uses tiktoken-rs for OpenAI models, character-based heuristic (chars/3.5) for others.

---

### §2.2 Configuration Management

**Satisfies:** GOAL-plat.2 (configuration file), GOAL-plat.16 (config migration)

#### Zero-Config Defaults

engram works out of the box with no config file. Defaults:

```rust
impl Default for PlatformConfig {
    fn default() -> Self {
        Self {
            llm: LlmConfig {
                provider: ProviderChoice::None,  // LLM features disabled, not broken
                task_models: HashMap::new(),
            },
            embeddings: EmbeddingConfig {
                model: EmbeddingModel::AllMiniLmL6V2,  // auto-download on first use
                cache_dir: None,  // defaults to ~/.cache/engram/models/
            },
            import: ImportConfig::default(),
            intake: IntakeConfig::default(),
        }
    }
}
```

When `provider: None`, all LLM-dependent features degrade gracefully (see §2.6). Core functionality (store, recall, topic discovery via embeddings) works without any LLM.

#### Config File Format

```toml
# ~/.config/engram/config.toml (or project-local engram.toml)

[llm]
provider = "openai"           # "openai" | "anthropic" | "local" | "none"
model = "gpt-4o-mini"         # default model
api_key_env = "OPENAI_API_KEY"  # read key from env var (never store raw key)

[llm.task_models]
topic_naming = "gpt-4o-mini"      # cheap model for short tasks
content_enhancement = "gpt-4o"    # quality model for long output

[embeddings]
model = "all-MiniLM-L6-v2"   # default, 384-dim, 80MB
# model = "bge-small-en-v1.5"  # alternative, 384-dim
cache_dir = "~/.cache/engram/models"

[import]
duplicate_strategy = "skip"   # "skip" | "update" | "append"
preserve_timestamps = true

[intake]
jina_api_key_env = "JINA_API_KEY"  # optional, for Jina Reader
```

#### Resolution Order

1. CLI flags (`--model`, `--provider`)
2. Environment variables (`ENGRAM_LLM_PROVIDER`, `ENGRAM_MODEL`)
3. Project-local `engram.toml` (current directory)
4. User config `~/.config/engram/config.toml`
5. Built-in defaults

```rust
pub fn load_config() -> PlatformConfig {
    let mut config = PlatformConfig::default();

    // Layer 4: user config
    if let Some(path) = user_config_path() {
        if path.exists() {
            config.merge_from_file(&path);
        }
    }
    // Layer 3: project-local
    if Path::new("engram.toml").exists() {
        config.merge_from_file(Path::new("engram.toml"));
    }
    // Layer 2: env vars
    config.merge_from_env();
    // Layer 1: CLI flags applied by caller

    config
}
```

#### Config Migration (GOAL-plat.16)

Config files carry a version number. On load, if the version is older than current,
the migration chain runs:

```rust
pub fn load_config_with_migration() -> PlatformConfig {
    let raw = load_raw_config();  // Parse TOML without schema validation
    let version = raw.get("version").and_then(|v| v.as_integer()).unwrap_or(0);
    
    if version < CURRENT_CONFIG_VERSION {
        // Backup old config
        let backup_path = config_path.with_extension(format!("toml.v{version}.bak"));
        fs::copy(&config_path, &backup_path)?;
        eprintln!("Config migrated from v{version} to v{CURRENT_CONFIG_VERSION}. Backup: {}", backup_path.display());
        
        // Run migration chain
        let migrated = migrate_config(raw, version, CURRENT_CONFIG_VERSION);
        migrated.write_to_file(&config_path)?;
    }
    
    parse_config(raw)
}

const CURRENT_CONFIG_VERSION: i64 = 1;

fn migrate_config(mut raw: toml::Value, from: i64, to: i64) -> toml::Value {
    for v in from..to {
        match v {
            0 => {
                // v0 → v1: add [embeddings] section, rename "model" → "llm.model"
                // ... migration logic
            }
            _ => {}
        }
    }
    raw["version"] = toml::Value::Integer(to);
    raw
}
```

---

### §2.3 Embedding Pipeline

**Satisfies:** GOAL-plat.4 (zero-config embedding setup), GOAL-plat.5 (embedding provider fallback)

#### Architectural Principle: Single Embedding Source

KC does NOT maintain its own embedding pipeline for source memories. Engram core already provides:
- `EmbeddingConfig` — provider selection (Ollama, OpenAI) with model and dimensions
- `Storage::store_embedding()` / `get_embedding()` / `get_all_embeddings()` — per-model embedding storage in `memory_embeddings` table
- `Storage::get_embeddings_in_namespace()` — namespace-scoped retrieval
- `Storage::find_nearest_embedding()` — vector similarity search

KC reads embeddings from this existing infrastructure. The flow is:
1. When a memory is stored (via `Memory::store()` or `engram add`), engram core computes and caches its embedding via the configured provider.
2. When KC needs embeddings for topic discovery or compilation, it calls `Storage::get_all_embeddings(model_id)` to load all pre-computed vectors.
3. KC's `MemorySnapshot` carries an `embedding: Option<Vec<f32>>` field — populated from `memory_embeddings` at load time.
4. When `embedding` is `None` (memory stored while embedding provider was offline), KC falls back to `simple_hash_embedding()` — a deterministic hash producing low-quality vectors suitable only for basic clustering.

This eliminates the duplication where `compiler/embedding.rs` defined its own `EmbeddingProvider` trait, `HttpEmbeddingProvider`, `StubEmbeddingProvider`, and `EmbeddingManager` — all redundant with engram core's `src/embeddings.rs`.

**KC's own embedding module (`compiler/embedding.rs`) is retained for:**
- Topic page embeddings (computing vectors for compiled topic pages, not source memories)
- `cosine_similarity()` utility
- `StubEmbeddingProvider` for testing

**KC does NOT use its own embedding module for:**
- Source memory embedding generation (reads from `memory_embeddings` instead)
- Embedding provider selection for source memories (uses engram core's `EmbeddingConfig`)

#### Embedding Cache (existing engram pattern)

Embeddings are stored alongside memories in SQLite (`memory_embeddings` table). The cache is:
- **Per-memory**: each memory row has a corresponding embedding row
- **Model-tagged**: PK is `(memory_id, model)`, so the same memory can have embeddings from multiple models
- **Lazy**: embeddings computed on `store()` and on-demand for older memories without embeddings

```rust
// Already implemented in storage.rs — KC reads from this, doesn't duplicate it
pub fn get_all_embeddings(&self, model: &str) -> Result<Vec<(String, Vec<f32>)>, rusqlite::Error>;
pub fn get_embedding(&self, memory_id: &str, model: &str) -> Result<Option<Vec<f32>>, rusqlite::Error>;
```

---

### §2.4 Import/Export

**Satisfies:** GOAL-plat.8 (Markdown import), GOAL-plat.9 (Obsidian import), GOAL-plat.10 (URL import), GOAL-plat.11 (bookmarks import), GOAL-plat.15 (progress & error reporting)

#### Import Architecture

```rust
/// Trait for format-specific importers.
pub trait Importer: Send + Sync {
    /// Parse a source into memory candidates.
    fn parse(&self, source: ImportSource) -> Result<Vec<MemoryCandidate>, ImportError>;

    /// Format name for logging/config.
    fn format_name(&self) -> &'static str;
}

pub enum ImportSource {
    File(PathBuf),
    Directory(PathBuf),
    Stdin,
}

/// Parsed but not-yet-stored memory.
pub struct MemoryCandidate {
    pub content: String,
    pub source_path: Option<PathBuf>,
    pub original_timestamp: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
    pub metadata: HashMap<String, String>,
    /// Content hash for dedup
    pub content_hash: String,
}
```

#### Importers

```rust
pub struct MarkdownImporter {
    /// Split strategy: per-file, per-heading, or per-paragraph
    pub split: SplitStrategy,
}

pub enum SplitStrategy {
    /// One memory per file
    PerFile,
    /// Split on ## headings
    PerHeading,
    /// Split on double newlines (paragraphs)
    PerParagraph,
}

pub struct LogseqImporter;   // Parses .md with `- ` block structure + `[[links]]`
pub struct ObsidianImporter;  // Parses .md with YAML frontmatter + `[[wikilinks]]`
pub struct JsonImporter;      // Expects [{content, timestamp, tags}, ...]
```

#### Duplicate Handling (GOAL-plat.11)

```rust
pub enum DuplicateStrategy {
    /// Skip if content_hash exists in DB
    Skip,
    /// Update existing memory with new content (keep ID, update timestamp)
    Update,
    /// Import as new memory even if duplicate exists
    Append,
}

pub struct ImportPipeline {
    importer: Box<dyn Importer>,
    strategy: DuplicateStrategy,
    embed_mgr: EmbeddingManager,
    storage: Storage,
    progress: Option<Box<dyn ImportProgress>>,
}

/// Progress callback for real-time feedback during batch imports (GOAL-plat.15).
pub trait ImportProgress: Send {
    fn on_item(&self, index: usize, total: usize, status: ItemStatus);
    fn on_complete(&self, report: &ImportReport);
}

pub enum ItemStatus {
    Imported { path: String },
    Skipped { path: String, reason: String },
    Failed { path: String, error: String },
}

impl ImportPipeline {
    pub async fn run(&mut self, source: ImportSource) -> Result<ImportReport, ImportError> {
        let candidates = self.importer.parse(source)?;
        let mut report = ImportReport::default();

        for candidate in candidates {
            // Check for exact duplicate (content hash)
            if let Some(existing) = self.storage.find_by_content_hash(&candidate.content_hash)? {
                match self.strategy {
                    DuplicateStrategy::Skip => {
                        report.skipped += 1;
                        continue;
                    }
                    DuplicateStrategy::Update => {
                        self.storage.update_memory(existing.id, &candidate)?;
                        report.updated += 1;
                        continue;
                    }
                    DuplicateStrategy::Append => { /* fall through to insert */ }
                }
            }

            // Check for near-duplicate (embedding similarity > 0.95)
            let embedding = self.embed_mgr.embed(&[&candidate.content]).await?;
            let similar = self.storage.find_similar(&embedding[0], 0.95, 1)?;
            if !similar.is_empty() {
                report.near_duplicates.push(NearDup {
                    candidate: candidate.content.clone(),
                    existing_id: similar[0].id,
                    similarity: similar[0].score,
                });
                // Near-dupes are reported but still imported (user decides)
            }

            let id = self.storage.store_memory(&candidate)?;
            self.storage.store_embedding(id, &embedding[0], self.embed_mgr.model.name())?;
            report.imported += 1;
        }

        Ok(report)
    }
}

pub struct ImportReport {
    pub imported: usize,
    pub skipped: usize,
    pub updated: usize,
    pub near_duplicates: Vec<NearDup>,
    pub errors: Vec<(String, ImportError)>,
}
```

---

### §2.5 Intake Pipeline

**Satisfies:** GOAL-plat.12 (directory watch intake), GOAL-plat.13 (voice intake), GOAL-plat.14 (browser extension)

Intake = import from URLs. Extracts content from web pages, processes into memories.

#### Architecture

```rust
pub struct IntakePipeline {
    extractors: Vec<Box<dyn ContentExtractor>>,
    import_pipeline: ImportPipeline,
}

#[async_trait]
pub trait ContentExtractor: Send + Sync {
    /// Check if this extractor handles the given URL.
    fn can_handle(&self, url: &Url) -> bool;

    /// Extract readable content from URL.
    async fn extract(&self, url: &Url) -> Result<ExtractedContent, IntakeError>;
}

pub struct ExtractedContent {
    pub title: String,
    pub author: Option<String>,
    pub content: String,     // cleaned text
    pub published: Option<DateTime<Utc>>,
    pub url: String,
    pub platform: String,
}

// Concrete extractors
pub struct JinaExtractor {
    api_key: Option<String>,  // optional — Jina has a free tier
}

pub struct YtDlpExtractor;    // YouTube via yt-dlp subtitle download
pub struct GithubExtractor;   // GitHub README via API
```

#### Processing Flow

```rust
impl IntakePipeline {
    pub async fn ingest(&mut self, url: &str) -> Result<IntakeReport, IntakeError> {
        let parsed = Url::parse(url)?;

        // Find matching extractor
        let extractor = self.extractors.iter()
            .find(|e| e.can_handle(&parsed))
            .ok_or(IntakeError::NoExtractor(url.to_string()))?;

        // Extract content
        let content = extractor.extract(&parsed).await?;

        // Convert to MemoryCandidate
        let candidate = MemoryCandidate {
            content: format!(
                "# {}\n\nSource: {}\nAuthor: {}\n\n{}",
                content.title,
                content.url,
                content.author.as_deref().unwrap_or("unknown"),
                content.content,
            ),
            source_path: None,
            original_timestamp: content.published,
            tags: vec![format!("intake:{}", content.platform)],
            metadata: [
                ("source_url".into(), content.url.clone()),
                ("platform".into(), content.platform.clone()),
            ].into(),
            content_hash: sha256_str(&content.url),  // dedup by URL
        };

        // Run through import pipeline (handles dedup, embedding, storage)
        let source = ImportSource::Memory(vec![candidate]);
        let report = self.import_pipeline.run(source).await?;

        Ok(IntakeReport {
            url: url.to_string(),
            title: content.title,
            import: report,
        })
    }
}
```

---

### §2.6 Graceful Degradation

**Satisfies:** GOAL-plat.14 (works without LLM), GOAL-plat.15 (fallback chains), GOAL-plat.16 (clear error messages)

#### Capability Levels

The system operates at three levels depending on available infrastructure:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CapabilityLevel {
    /// No LLM, no embeddings — basic store/recall only
    Minimal,
    /// Embeddings available, no LLM — semantic search + clustering works
    Embeddings,
    /// Full — LLM + embeddings — all features including enhancement
    Full,
}

impl CapabilityLevel {
    pub fn detect(config: &PlatformConfig) -> Self {
        let has_llm = config.llm.provider != ProviderChoice::None;
        let has_embed = config.embeddings.model != EmbeddingModel::None;

        match (has_llm, has_embed) {
            (true, true) => Self::Full,
            (false, true) => Self::Embeddings,
            _ => Self::Minimal,
        }
    }
}
```

#### Feature Availability Matrix

```
Feature                    | Minimal | Embeddings | Full
---------------------------|---------|------------|------
store/recall (text match)  |   ✅    |    ✅      |  ✅
semantic search            |   ❌    |    ✅      |  ✅
topic discovery            |   ❌    |    ✅      |  ✅
topic rendering (basic)    |   ❌    |    ✅      |  ✅
topic enhancement (LLM)    |   ❌    |    ❌      |  ✅
conflict analysis (LLM)    |   ❌    |    ❌      |  ✅
smart topic naming (LLM)   |   ❌    |    ❌      |  ✅
```

#### Fallback Chain (GOAL-plat.15)

```rust
pub struct FallbackProvider {
    primary: Box<dyn LlmProvider>,
    fallbacks: Vec<Box<dyn LlmProvider>>,
}

#[async_trait]
impl LlmProvider for FallbackProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        match self.primary.complete(request).await {
            Ok(resp) => Ok(resp),
            Err(e) => {
                eprintln!("Primary LLM failed: {e}, trying fallbacks...");
                for fallback in &self.fallbacks {
                    match fallback.complete(request).await {
                        Ok(resp) => return Ok(resp),
                        Err(e2) => {
                            eprintln!("Fallback {} failed: {e2}", fallback.metadata().name);
                            continue;
                        }
                    }
                }
                Err(LlmError::AllProvidersFailed)
            }
        }
    }
}
```

#### Error Messages (GOAL-plat.3)

Every degradation produces a user-visible, actionable message:

```rust
pub fn degradation_message(feature: &str, level: CapabilityLevel) -> String {
    match level {
        CapabilityLevel::Minimal => format!(
            "⚠️ {feature} requires embeddings. Run `engram setup` to download \
             a model (80MB, one-time), or set embeddings.model in config."
        ),
        CapabilityLevel::Embeddings => format!(
            "ℹ️ {feature} works but without LLM enhancement. \
             Set llm.provider in ~/.config/engram/config.toml for full features."
        ),
        CapabilityLevel::Full => unreachable!(),
    }
}
```

---

### §2.7 Installation & Setup

**Satisfies:** GOAL-plat.6 (standalone product installation)

KC as a standalone product (no RustClaw dependency) with guided first-run setup.

#### Distribution Channels

```
cargo install engramai          # from crates.io (requires Rust toolchain)
brew install engramai            # macOS via Homebrew formula
curl -sSf https://... | sh       # pre-built binary installer (macOS arm64/x86, Linux x86)
```

#### First-Run Setup (`engram init`)

```rust
pub async fn init_wizard(opts: InitOpts) -> Result<PlatformConfig, InitError> {
    // 1. Database location
    let db_path = opts.db_path.unwrap_or_else(|| {
        let default = dirs::data_dir().unwrap().join("engram/knowledge.db");
        prompt_with_default("Database path", &default)
    });

    // 2. Embedding runtime
    //    Check if already available, offer auto-install if not
    let embed_config = setup_embeddings().await?;

    // 3. LLM provider (optional)
    let llm_config = if prompt_yn("Configure LLM provider? (optional, enables compilation)") {
        setup_llm_provider()?
    } else {
        LlmConfig { provider: ProviderChoice::None, ..Default::default() }
    };

    // 4. Write config file
    let config = PlatformConfig { db_path, llm: llm_config, embeddings: embed_config, ..Default::default() };
    let config_path = dirs::config_dir().unwrap().join("engram/config.toml");
    config.write_to_file(&config_path)?;

    // 5. Verify: store + recall a test memory
    verify_setup(&config).await?;

    println!("✅ Setup complete. Config: {}", config_path.display());
    Ok(config)
}
```

#### Platform Support Matrix

| Platform | Auto-install | Pre-built binary | Notes |
|----------|-------------|-----------------|-------|
| macOS arm64 | ✅ | ✅ | Primary target |
| macOS x86 | ✅ | ✅ | |
| Linux x86_64 | ✅ | ✅ | |
| Linux arm64 | ❌ manual | ✅ | Embedding runtime may need manual setup |
| Windows | ❌ manual | ❌ | Not a target for P0 |

---

### §2.8 Feature Flag Architecture

**Satisfies:** GOAL-plat.7 (feature flags for KC vs agent separation)

Compile-time feature gates in `Cargo.toml` separating KC product features from agent-specific features.

```toml
# In engramai/Cargo.toml
[features]
# KC standalone features (default = knowledge product)
default = ["storage", "recall", "embeddings", "fts5", "entities", "synthesis", "compiler"]

# Core
storage = []
recall = ["storage"]

# Knowledge pipeline
embeddings = ["storage"]        # vector embeddings via ONNX runtime
fts5 = ["storage"]              # full-text search
entities = []                    # entity extraction (Aho-Corasick + regex)
synthesis = ["embeddings"]       # cluster discovery, Hebbian analysis
compiler = ["synthesis", "fts5", "entities"]  # KC compilation pipeline

# Agent-specific (opt-in, NOT in default)
session = ["storage"]            # session/working memory management
extractor = []                   # LLM-based fact extraction
classifier = ["embeddings"]      # query classification
anomaly = ["embeddings"]         # anomaly detection
sentiment = []                   # sentiment analysis
```

```rust
// In src/lib.rs — conditional module declarations
pub mod storage;                           // always

#[cfg(feature = "recall")]
pub mod memory;                            // recall

#[cfg(feature = "embeddings")]
pub mod embeddings;                        // embeddings

#[cfg(feature = "compiler")]
pub mod compiler;                          // KC compilation

#[cfg(feature = "session")]
pub mod session;                           // agent working memory

#[cfg(feature = "extractor")]
pub mod extractor;                         // LLM fact extraction
```

The standalone KC binary enables only `default` features. Agent frameworks (RustClaw, OpenClaw) enable additional features as needed.

---

## §3 Data Flow

```
CLI / Library API
       │
       ▼
┌─────────────────┐
│  load_config()  │  §2.2 — resolve TOML + env + defaults
└───────┬─────────┘
        │
        ├──────────────────────┐
        ▼                      ▼
┌───────────────┐    ┌──────────────────┐
│ ModelRouter    │    │ EmbeddingManager │
│ (§2.1)        │    │ (§2.3)           │
│ LLM requests  │    │ auto-download    │
│ task routing   │    │ encode texts     │
└───────┬───────┘    └────────┬─────────┘
        │                     │
        └──────────┬──────────┘
                   ▼
        ┌──────────────────┐
        │  Compilation /   │  (consumes LLM + embeddings)
        │  Maintenance     │
        └──────────────────┘

Import/Intake (§2.4, §2.5):
  External source → Importer/Extractor → MemoryCandidate → Storage + Embedding
```

---

## §4 Integration Points

### With Compilation Feature
- Compilation calls `ModelRouter::for_task(TopicNaming)` and `for_task(ContentEnhancement)`
- Compilation calls `EmbeddingManager::embed()` for clustering
- Platform owns the `LlmProvider` instances; compilation borrows them

### With Maintenance Feature
- Maintenance calls `ModelRouter::for_task(ConflictAnalysis)`
- Maintenance calls `EmbeddingManager::embed()` for duplicate detection
- Health checks use `LlmProvider::health_check()` to report platform status

### With Existing engram Code
- `EmbeddingManager` replaces the current inline embedding logic in `embeddings.rs`
- `Storage` extensions (content_hash column, embedding model tag) are additive — no schema breaks
- `PlatformConfig` extends existing `EngineConfig` — old configs remain valid
- **`LlmProvider` is the single LLM abstraction.** The existing `SynthesisLlmProvider` in `synthesis/mod.rs` will be replaced by this trait. Compilation and maintenance code programs against `LlmProvider` (via `ModelRouter::for_task()`) — they never reference `SynthesisLlmProvider` directly.

---

## §5 Requirements Traceability

| GOAL | Component | How |
|------|-----------|-----|
| GOAL-plat.1 | §2.1 LlmProvider trait | Trait abstraction, 3 provider impls |
| GOAL-plat.2 | §2.1 ModelRouter + §2.2 Config | Task→model mapping via config file |
| GOAL-plat.3 | §2.6 Graceful Degradation | Three-tier capability levels, actionable error messages |
| GOAL-plat.4 | §2.3 ensure_model() | Auto-download with progress + checksum |
| GOAL-plat.5 | §2.3 + §2.6 FallbackProvider | Local → Cloud → keyword-only fallback chain |
| GOAL-plat.6 | §2.7 Installation & Setup | `engram init` wizard, multi-platform distribution |
| GOAL-plat.7 | §2.8 Feature Flags | Cargo feature gates: default (KC) vs opt-in (agent) |
| GOAL-plat.8 | §2.4 MarkdownImporter | Per-file/heading/paragraph split |
| GOAL-plat.9 | §2.4 ObsidianImporter | Wikilinks → Hebbian links, YAML frontmatter → metadata |
| GOAL-plat.10 | §2.4 URL import | Batch URL list → fetch → import with rate limiting |
| GOAL-plat.11 | §2.4 Bookmarks import | Chrome/Firefox bookmark file parser |
| GOAL-plat.12 | §2.5 IntakePipeline | Inbox directory watcher, auto-import on file drop |
| GOAL-plat.13 | §2.5 Voice intake | STT (.ogg/.wav/.mp3) → text → memory |
| GOAL-plat.14 | §2.5 Browser extension | HTTP endpoint for browser extension content capture |
| GOAL-plat.15 | §2.4 ImportProgress trait | Real-time progress callback + consistent summary report |
| GOAL-plat.16 | §2.2 Config migration | Version detection, migration chain, backup before migrate |

