# Architecture: Knowledge Compiler (engram-ai-rust)

> Master architecture document. Covers system-wide structure, cross-cutting concerns,
> data flow between features, shared types, and feature index.
> Feature-level designs live in `.gid/features/{feature}/design*.md`.

## §1 Architecture Overview

### 1.1 System Purpose

Knowledge Compiler transforms engram's flat memory store (thousands of individual Memory rows in SQLite) into structured, browsable knowledge artifacts — topic pages, timelines, and cross-referenced knowledge graphs. It operates as a library crate (`engramai`) with a CLI surface (`engram`).

### 1.2 Architectural Style

**Pipeline architecture** with three layers:

```
┌─────────────────────────────────────────────────────────┐
│                    CLI / API Surface                     │
│            (main.rs — commands, I/O formatting)          │
├─────────────────────────────────────────────────────────┤
│                  Compilation Pipeline                    │
│  ┌───────────┐  ┌───────────┐  ┌──────────┐  ┌───────┐ │
│  │  Topic    │→ │ Rendering │→ │ Feedback │→ │ Maint │ │
│  │ Discovery │  │ & Output  │  │ & Refine │  │ enance│ │
│  └───────────┘  └───────────┘  └──────────┘  └───────┘ │
├─────────────────────────────────────────────────────────┤
│                   Foundation Layer                        │
│  ┌──────────┐ ┌──────────┐ ┌─────────┐ ┌─────────────┐ │
│  │ Storage  │ │Embeddings│ │ Entities│ │ LLM Provider│ │
│  │ (SQLite) │ │ (Vector) │ │ Extract │ │ (Multi)     │ │
│  └──────────┘ └──────────┘ └─────────┘ └─────────────┘ │
└─────────────────────────────────────────────────────────┘
```

Data flows top-down for queries, bottom-up for compilation. The foundation layer is shared by all features and already exists in the codebase.

### 1.3 Component Inventory (≤8 components)

| # | Component | Location | Status | Primary Feature |
|---|-----------|----------|--------|-----------------|
| 1 | **Storage** | `src/storage.rs` | Exists | All (foundation) |
| 2 | **Embeddings** | `src/embeddings.rs` | Exists | All (foundation) |
| 3 | **Entities & Extraction** | `src/entities.rs`, `src/extractor.rs` | Exists | All (foundation) |
| 4 | **Synthesis Engine** | `src/synthesis/` | Exists | Compilation |
| 5 | **Topic Compiler** | `src/compiler/` | New | Compilation |
| 6 | **Knowledge Maintenance** | `src/maintenance/` | New | Maintenance |
| 7 | **LLM Provider** | `src/config.rs`, `src/models/` | Exists | All (foundation) |
| 8 | **CLI Surface** | `src/main.rs` | Exists (extend) | Platform |

### 1.4 Key Design Decisions

**D1: Extend, don't replace.** The existing synthesis engine (`src/synthesis/`) already does cluster discovery, gate checking, insight generation, and provenance tracking. Topic Compiler builds *on top of* synthesis — it consumes `SynthesizedInsight` as input, not raw memories. This avoids duplicating the clustering/gating logic.

**D2: SQLite-only persistence.** All new tables (topics, compilations, health metrics) go into the same SQLite database. No new storage backends. The existing `Storage` struct gains new methods via `impl` blocks or a `TopicStore` trait.

**D3: LLM calls go through existing `LlmProvider`.** The multi-provider config (GOAL-plat.1 through plat.4) extends `src/config.rs` and `src/models/`. No new LLM abstraction. Topic rendering, conflict detection, and maintenance all use the same provider interface.

**D4: Offline-first with optional LLM.** Every operation has a non-LLM fallback path. Topic discovery works via entity co-occurrence + embedding clustering without LLM. Rendering falls back to template-based output. Only quality improves with LLM, not availability.

**D5: Incremental by default.** Recompilation (GOAL-comp.5) uses content hashing — each memory has a hash, each topic page records the hashes of its source memories. On recompile, only topics whose source hashes changed are regenerated. Full recompile is opt-in.

---

## §2 Cross-Cutting Concerns

### 2.1 Error Handling

All new modules follow the existing pattern in `src/synthesis/engine.rs`:

- `EngineError` enum with typed variants per failure mode
- `Result<T, EngineError>` on all public APIs
- LLM failures → graceful degradation (return template/fallback, not error)
- Storage failures → propagate up (caller decides retry/abort)
- No `.unwrap()` on user data; `.unwrap()` only on compile-time invariants

### 2.2 LLM Abstraction

All LLM calls go through a single trait boundary:

```rust
// Existing in src/models/
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn generate(&self, prompt: &str, max_tokens: u32) -> Result<String, LlmError>;
}
```

New compilation/maintenance code receives `&dyn LlmProvider` — never constructs its own client. This satisfies GUARD-1 (no API lock-in) and GUARD-7 (testability — tests inject a mock provider).

### 2.3 Privacy & Access Control

Per GUARD-4 (privacy-by-default):

- All topic pages inherit the most restrictive access level of their source memories
- `AccessLevel` enum: `Private | Shared | Public` — stored per topic
- Compilation never sends memory content to external services beyond the configured LLM provider
- Export functions strip private memories unless explicitly included
- No telemetry, no analytics, no external calls beyond LLM

### 2.4 Performance Budgets

Per GUARD-5 (resource-conscious):

- Topic discovery scan: O(n) in memory count, target <2s for 10k memories
- Single topic compilation: <5s including one LLM call
- Incremental recompile: proportional to changed memories, not total
- Memory baseline: <50MB RSS for 10k memories (SQLite + embedding cache)
- All batch operations support `--limit N` to cap resource usage

### 2.5 Testability

Per GUARD-7 (testable-without-LLM):

- Core logic is pure: `fn compile(memories: &[Memory], config: &CompileConfig) -> TopicPage`
- LLM enhancement is a separate pass: `fn enhance(page: &mut TopicPage, llm: &dyn LlmProvider)`
- All modules testable with `MockLlmProvider` that returns canned responses
- Integration tests use an in-memory SQLite database
- Existing test pattern: `#[cfg(test)] mod tests` in each source file

### 2.6 Configuration

Extends the existing `EngineConfig` (in `src/config.rs`):

```rust
pub struct KcConfig {
    /// LLM provider settings (existing)
    pub llm: LlmConfig,
    /// Minimum memories to form a topic (default: 3)
    pub min_topic_size: usize,
    /// Recompile strategy: incremental or full
    pub recompile_mode: RecompileMode,
    /// Health check schedule (default: on consolidate)
    pub maintenance_schedule: MaintenanceSchedule,
    /// Privacy default for new topics
    pub default_access: AccessLevel,
}
```

All settings have sensible defaults → zero-config works (GOAL-plat.7).

### 2.7 Migration & Backward Compatibility

Per GUARD-6 (backward-compatible):

- New SQLite tables are additive — no existing table schema changes
- Existing `engram` CLI commands unchanged
- New commands added under `engram kc` namespace (or `engram compile`, `engram topics`)
- Library API: new functions added, no existing function signatures changed
- If a database has no KC tables, they're created lazily on first KC operation

---

## §3 Data Flow Between Features

### 3.1 End-to-End Pipeline

```
 User input (text, URL, note)
       │
       ▼
 ┌─────────────┐
 │  Ingestion   │  src/memory.rs — store(), entities.rs — extract()
 │  (existing)  │  Creates: Memory row + entity links + embedding
 └──────┬──────┘
        │
        ▼
 ┌─────────────┐
 │  Synthesis   │  src/synthesis/ — cluster, gate, generate insight
 │  (existing)  │  Creates: SynthesizedInsight rows + provenance links
 └──────┬──────┘
        │
        ▼
 ┌─────────────────┐
 │ Topic Discovery  │  src/compiler/discovery.rs — NEW
 │                  │  Input: memories + entities + insights + embeddings
 │                  │  Output: TopicCandidate list
 └───────┬─────────┘
         │
         ▼
 ┌─────────────────┐
 │ Topic Rendering  │  src/compiler/render.rs — NEW
 │                  │  Input: TopicCandidate + source memories
 │                  │  Output: TopicPage (structured markdown + metadata)
 │                  │  Optional: LLM enhancement pass
 └───────┬─────────┘
         │
         ▼
 ┌─────────────────┐
 │  Feedback Loop   │  src/compiler/feedback.rs — NEW
 │                  │  Input: user rating/correction on TopicPage
 │                  │  Output: updated TopicPage + adjusted weights
 └───────┬─────────┘
         │
         ▼
 ┌─────────────────┐
 │   Maintenance    │  src/maintenance/ — NEW
 │                  │  Runs periodically or on-demand
 │                  │  Detects: stale topics, conflicts, broken links, dupes
 │                  │  Output: HealthReport + auto-fixes
 └─────────────────┘
```

### 3.2 Feature Interaction Matrix

```
                  Compilation    Maintenance    Platform
                  ───────────    ───────────    ────────
Compilation          —           provides       uses LLM
                                 topics to      config
                                 maintain

Maintenance       triggers       —              uses LLM
                  recompile                     config,
                  on fixes                      storage

Platform          provides       provides       —
                  LLM +          storage +
                  storage        embeddings
```

### 3.3 Data Boundaries

**Compilation reads, never mutates source data.** Topic compilation reads Memory rows and SynthesizedInsight rows but never modifies them. It writes only to topic-related tables. This is a hard invariant — compilation is a read-from-source, write-to-output pipeline.

**Maintenance may trigger recompilation.** When maintenance detects stale content (source memories changed), it marks affected topics as `stale` and optionally triggers incremental recompile. It does NOT directly edit topic content — it goes through the compilation pipeline.

**Platform is pure infrastructure.** LLM config, embedding provider, storage layer — these are consumed by compilation and maintenance, never the reverse. Platform has no knowledge of topic semantics.

---

## §4 Shared Types

These types are used across multiple features and must be defined in a shared location (`src/types.rs` or `src/compiler/types.rs`).

### 4.1 Core Types

```rust
/// Unique identifier for a compiled topic.
/// Format: deterministic hash of canonical topic name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TopicId(pub String);

/// A discovered topic candidate before compilation.
#[derive(Debug, Clone)]
pub struct TopicCandidate {
    /// Suggested topic name (may be refined by LLM)
    pub name: String,
    /// Memory IDs that belong to this topic
    pub memory_ids: Vec<i64>,
    /// Entity IDs that define this topic
    pub entity_ids: Vec<String>,
    /// Discovery confidence score [0.0, 1.0]
    pub confidence: f64,
    /// How the topic was discovered
    pub discovery_method: DiscoveryMethod,
}

#[derive(Debug, Clone)]
pub enum DiscoveryMethod {
    /// Entity co-occurrence clustering
    EntityCluster,
    /// Embedding similarity clustering
    EmbeddingCluster,
    /// LLM-assisted discovery
    LlmAssisted,
    /// User-defined manual topic
    Manual,
}

/// A compiled topic page — the primary output artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicPage {
    pub id: TopicId,
    pub title: String,
    /// Rendered content (markdown)
    pub content: String,
    /// Structured summary (1-3 sentences)
    pub summary: String,
    /// Source memory IDs (provenance)
    pub source_memory_ids: Vec<i64>,
    /// Source insight IDs (from synthesis)
    pub source_insight_ids: Vec<String>,
    /// Content hash for incremental recompile detection
    pub content_hash: String,
    /// When this page was last compiled
    pub compiled_at: chrono::DateTime<chrono::Utc>,
    /// Access control
    pub access: AccessLevel,
    /// Compilation metadata
    pub metadata: TopicMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicMetadata {
    /// Number of times recompiled
    pub revision: u32,
    /// User feedback score (None = no feedback yet)
    pub user_rating: Option<f32>,
    /// Whether LLM was used in compilation
    pub llm_enhanced: bool,
    /// Staleness: true if source memories changed since last compile
    pub stale: bool,
}
```

### 4.2 Access Control

```rust
/// Privacy level for topic pages.
/// Inherits from the most restrictive source memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessLevel {
    /// Only the owner can see (default)
    Private,
    /// Shared with specific contexts
    Shared,
    /// Publicly visible
    Public,
}

impl Default for AccessLevel {
    fn default() -> Self {
        Self::Private
    }
}
```

### 4.3 Health & Maintenance Types

```rust
/// Result of a maintenance health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub checked_at: chrono::DateTime<chrono::Utc>,
    pub total_topics: usize,
    pub stale_topics: Vec<TopicId>,
    pub conflicts: Vec<ConflictRecord>,
    pub broken_links: Vec<BrokenLink>,
    pub duplicates: Vec<DuplicateGroup>,
    pub overall_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictRecord {
    pub topic_id: TopicId,
    pub description: String,
    pub conflicting_memory_ids: (i64, i64),
    pub severity: ConflictSeverity,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ConflictSeverity {
    /// Direct contradiction — must resolve
    Hard,
    /// Tension or ambiguity — should review
    Soft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokenLink {
    pub source_topic: TopicId,
    pub target: String,
    pub link_type: LinkType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LinkType {
    TopicCrossRef,
    MemoryRef,
    EntityRef,
    ExternalUrl,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub canonical: TopicId,
    pub duplicates: Vec<TopicId>,
    pub similarity: f64,
}
```

### 4.4 Recompile Types

```rust
#[derive(Debug, Clone, Copy)]
pub enum RecompileMode {
    /// Only recompile topics whose source memories changed (default)
    Incremental,
    /// Recompile all topics regardless of changes
    Full,
}

/// Tracks what changed to decide recompilation scope.
#[derive(Debug, Clone)]
pub struct ChangeSet {
    /// Memory IDs that were added/modified since last compile
    pub changed_memory_ids: Vec<i64>,
    /// Topic IDs affected by those changes
    pub affected_topic_ids: Vec<TopicId>,
}
```

---

## §5 Feature Index

Each feature has its own requirements and design documents under `.gid/features/`.

### Feature 1: Topic Compilation & Feedback

- **Requirements**: `.gid/features/knowledge-compiler/compilation/requirements.md`
- **Design**: `.gid/features/knowledge-compiler/compilation/design.md` (to be written)
- **Scope**: Topic discovery, rendering, incremental recompilation, user feedback loop
- **GOALs**: GOAL-comp.1 through GOAL-comp.8
- **Components**: Topic Compiler (`src/compiler/`)
- **Depends on**: Storage, Embeddings, Entities, Synthesis Engine, LLM Provider

### Feature 2: Knowledge Maintenance, Access & Privacy

- **Requirements**: `.gid/features/knowledge-compiler/maintenance/requirements.md`
- **Design**: `.gid/features/knowledge-compiler/maintenance/design.md` (to be written)
- **Scope**: Decay detection, conflict detection, broken link repair, duplicate detection, health reporting, access control, export
- **GOALs**: GOAL-maint.1 through GOAL-maint.10
- **Components**: Knowledge Maintenance (`src/maintenance/`)
- **Depends on**: Storage, Embeddings, Topic Compiler (reads topic pages)

### Feature 3: Platform Setup, LLM Config, Import & Intake

- **Requirements**: `.gid/features/knowledge-compiler/platform/requirements.md`
- **Design**: `.gid/features/knowledge-compiler/platform/design.md` (to be written)
- **Scope**: Multi-provider LLM config, zero-config setup, embedding auto-install, import from external sources, intake pipeline
- **GOALs**: GOAL-plat.1 through GOAL-plat.12
- **Components**: LLM Provider (`src/config.rs`, `src/models/`), CLI Surface (`src/main.rs`)
- **Depends on**: Storage, Embeddings

### Implementation Order

```
Phase 1: Platform (LLM config, embeddings, import)
    ↓  — foundation must exist before compilation can use LLM
Phase 2: Compilation (discovery, rendering, feedback)
    ↓  — topics must exist before maintenance can check them
Phase 3: Maintenance (health, conflicts, decay, export)
```

This order follows the data flow: platform provides infrastructure → compilation produces artifacts → maintenance keeps artifacts healthy.

---

## Appendix A: Existing Code Map

For reference — modules that already exist and will be extended or consumed:

| Module | Lines | Role in KC |
|--------|-------|-----------|
| `storage.rs` | 104k | Add topic tables, topic CRUD methods |
| `memory.rs` | 132k | Memory ingestion — unchanged, read-only for KC |
| `entities.rs` | 22k | Entity extraction — consumed by topic discovery |
| `extractor.rs` | 20k | LLM-based fact extraction — consumed by compilation |
| `embeddings.rs` | 18k | Vector embeddings — consumed by clustering |
| `synthesis/` | ~60k | Cluster + gate + insight — foundation for topic discovery |
| `config.rs` | 10k | Extend with KC-specific config |
| `hybrid_search.rs` | 14k | Search — consumed by maintenance (duplicate detection) |
| `lib.rs` | 4k | Public API — extend with KC exports |
| `main.rs` | 60k | CLI — add KC subcommands |

## Appendix B: Non-Goals

These are explicitly **not** in scope for this architecture:

- **Real-time collaboration** — single-user system
- **Web UI** — CLI and library API only; UI is a separate project
- **Distributed storage** — single SQLite file, no replication
- **Custom embedding training** — uses pre-trained models only
- **Plugin system** — no dynamic loading; extend via Rust code
