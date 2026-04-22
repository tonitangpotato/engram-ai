# Design: Knowledge Maintenance, Access & Privacy

> Feature-level design for the maintenance subsystem: knowledge decay, conflict
> detection, health reporting, export/import, CLI/API access, and privacy controls.
> Parent architecture: `../../docs/architecture.md`

## 1. Overview

This feature keeps compiled knowledge healthy over time and provides interfaces for
humans and external systems to access it. The maintenance subsystem runs as a
background process (triggered by the scheduler or on-demand via CLI/API) that
evaluates knowledge freshness, detects conflicts, repairs broken links, identifies
duplicates, and produces health reports. Separately, the access layer exposes
compiled knowledge through a CLI and programmatic API, while the privacy layer
enforces visibility and redaction rules before any knowledge leaves the system.

### Goals (from requirements)

| ID | Summary |
|----|--------|
| GOAL-maint.1 | ACT-R activation-derived decay with configurable thresholds |
| GOAL-maint.2 | Conflict detection across overlapping topics |
| GOAL-maint.3 | Broken source-link detection and repair/tombstone |
| GOAL-maint.4 | Duplicate/near-duplicate topic detection and merge suggestions |
| GOAL-maint.5 | Health reports with per-topic scores and system-wide metrics |
| GOAL-maint.5b | Per-operation summaries (counts, timing, token cost) |
| GOAL-maint.6 | Knowledge-aware recall (topic pages participate in recall ranking) |
| GOAL-maint.7 | Markdown export compatible with Obsidian |
| GOAL-maint.8 | CLI subcommands for all operations |
| GOAL-maint.9 | Programmatic Rust API mirroring CLI capabilities |
| GOAL-maint.10 | Local data sovereignty (no network except LLM API) |
| GOAL-maint.11 | LLM data transparency (verbose prompt logging) |
| GOAL-maint.12 | Optional DB encryption at rest |

### Non-goals

- Real-time streaming subscriptions to knowledge changes (batch export is sufficient)
- Multi-user access control / RBAC (single-user agent system)
- Automatic conflict resolution without human review

## 2. Components

This design uses **7 components** (within the ≤8 limit).

### §2.1 Decay Engine

**Satisfies**: GOAL-maint.1

Evaluates freshness of compiled topics and their source links. Runs as a
`MaintenanceTask` dispatched by the architecture's `MaintenanceScheduler`.

```
DecayEngine
├── evaluate_topic(topic_id) -> DecayResult
├── evaluate_all() -> Vec<DecayResult>
└── apply_decay(topic_id, action: DecayAction) -> Result<()>
```

**Decay model**: Each `CompiledTopic` carries a `freshness_score: f32` (0.0–1.0).
The score is **derived from source memories' ACT-R activation** (per GOAL-maint.1),
not an independent decay formula:

```
freshness = weighted_mean(source_memory.act_r_activation for each source_memory)
```

Where `weighted_mean` uses recency weighting—more recently added source memories
contribute more to the freshness score. This leverages engram's existing ACT-R
model which already captures both time decay and access patterns (recall = Hebbian
strengthening = higher activation).

**Freshness recomputation**: Triggered when:
- A source memory is recalled (ACT-R activation boosted via Hebbian strengthening)
- Time passes (ACT-R base-level activation naturally decays)
- A source memory is deleted (removed from weighted average)

Batch recomputation runs during scheduled maintenance cycles.
Per-topic recomputation runs on any source memory recall event.

Configuration lives in `MaintenanceConfig`:

```rust
pub struct DecayConfig {
    /// Freshness threshold below which topic is flagged stale (default: 0.3)
    pub staleness_threshold: f32,
    /// Freshness threshold below which topic is candidate for archival (default: 0.1)
    pub archival_threshold: f32,
    /// Recency weight exponent for weighted_mean (default: 1.5)
    /// Higher values = more recent source memories dominate the score
    pub recency_weight_exponent: f32,
}
```

**DecayAction** enum:
- `None` — score above staleness threshold, no action
- `FlagStale` — score below staleness threshold, mark for review
- `Archive` — score below archival threshold, move to archived state
- `Boost(f32)` — manual freshness boost (e.g., after user access resets decay)

Accessing a topic via query resets its access-decay clock (last_accessed timestamp
updated). This is the "access-based" signal: frequently-queried topics stay fresh.

### §2.2 Conflict Detector

**Satisfies**: GOAL-maint.2, GOAL-maint.4

Finds contradictions between overlapping topics and identifies near-duplicate topics
that should be merged.

```
ConflictDetector
├── detect_conflicts(scope: ConflictScope) -> Vec<Conflict>
├── detect_duplicates(threshold: f32) -> Vec<DuplicateGroup>
└── suggest_resolutions(conflict_id) -> Vec<Resolution>
```

**Conflict detection strategy** (two-phase):

1. **Candidate selection** — For each topic, find other topics with overlapping
   source memories (shared `source_memory_ids`) or high embedding similarity
   (cosine > 0.85 on topic summary embeddings). This narrows the O(n²) space.

2. **LLM contradiction check** — For each candidate pair, send both compiled
   summaries to the LLM with a structured prompt asking:
   - Do these topics make contradictory claims? (yes/no + evidence)
   - Are these topics near-duplicates that should be merged? (yes/no + overlap %)

```rust
pub struct Conflict {
    pub id: ConflictId,
    pub topic_a: TopicId,
    pub topic_b: TopicId,
    pub conflict_type: ConflictType,
    pub description: String,
    pub evidence: Vec<String>,
    pub suggested_resolution: Option<Resolution>,
    pub detected_at: DateTime<Utc>,
    pub status: ConflictStatus,
}

pub enum ConflictType {
    Contradiction,
    NearDuplicate { overlap_pct: f32 },
    StaleOverlap,
}

pub enum ConflictStatus {
    Open,
    ResolvedMerge,
    ResolvedKeepBoth { rationale: String },
    Dismissed,
}
```

**Duplicate detection** reuses the candidate-selection phase but with a lower
similarity threshold (cosine > 0.80). Duplicates are grouped into `DuplicateGroup`s
and surfaced in health reports. Merge is always human-approved (via CLI confirm or
API call), never automatic.

**LLM cost awareness (GUARD-3)**: Before sending candidate pairs to the LLM for
contradiction checking, the ConflictDetector estimates token count from the two
summaries' lengths. If estimated cost exceeds `KcConfig.budget_threshold`, the
LLM check is skipped and the pair is flagged as "candidate conflict (LLM budget
exceeded, manual review needed)". Token usage for completed LLM calls is tracked
in `OperationSummary`.

**ConflictScope** controls what to scan:
- `All` — full pairwise scan (expensive, for scheduled deep maintenance)
- `RecentlyChanged(Duration)` — only topics compiled/recompiled in the window
- `SingleTopic(TopicId)` — check one topic against all others

### §2.3 Link & Health Auditor

**Satisfies**: GOAL-maint.3, GOAL-maint.5

Validates source-link integrity and produces health reports.

```
HealthAuditor
├── audit_links(scope: AuditScope) -> LinkAuditReport
├── repair_link(link_id, action: LinkRepairAction) -> Result<()>
├── health_report(scope: ReportScope) -> HealthReport
└── topic_score(topic_id) -> TopicHealthScore
```

**Broken link detection**: Each `CompiledTopic` references source memories by ID.
The auditor verifies each reference still exists in the engram store and that the
source content hasn't changed since compilation (via content hash comparison).

```rust
pub enum LinkStatus {
    Valid,
    SourceDeleted,
    SourceModified { old_hash: u64, new_hash: u64 },
    SourceInaccessible { reason: String },
}

pub enum LinkRepairAction {
    /// Mark link as dead, keep topic but note reduced confidence
    Tombstone,
    /// Trigger recompilation with updated source
    Recompile,
    /// Remove this source from the topic (if other sources remain)
    Detach,
}
```

**Health report** aggregates per-topic scores into a system-wide view:

```rust
pub struct HealthReport {
    pub generated_at: DateTime<Utc>,
    pub total_topics: usize,
    pub healthy: usize,       // freshness > staleness_threshold, no broken links
    pub stale: usize,         // freshness < staleness_threshold
    pub conflicted: usize,    // has open conflicts
    pub broken_links: usize,  // has ≥1 broken source link
    pub duplicate_groups: usize,
    pub per_topic: Vec<TopicHealthScore>,
    pub recommendations: Vec<MaintenanceRecommendation>,
}

pub struct TopicHealthScore {
    pub topic_id: TopicId,
    pub freshness: f32,
    pub source_integrity: f32,  // fraction of valid source links
    pub conflict_count: usize,
    pub overall: f32,           // weighted combination
}
```

**MaintenanceRecommendation** is an actionable item:
- "Topic X has freshness 0.08 — consider archival"
- "Topics A and B are 92% similar — consider merging"
- "Topic C has 3/5 broken source links — recompile or tombstone"

### §2.4 Export/Import Engine

**Satisfies**: GOAL-maint.6

Serializes compiled knowledge into portable formats and imports from them.

```
ExportEngine
├── export(filter: ExportFilter, format: ExportFormat, dest: Path) -> Result<ExportManifest>
├── import(source: Path, policy: ImportPolicy) -> Result<ImportReport>
└── supported_formats() -> Vec<ExportFormat>
```

**Export formats**:

```rust
pub enum ExportFormat {
    /// JSON array of CompiledTopic with metadata — lossless round-trip
    Json,
    /// Markdown files, one per topic, in a directory tree by category
    Markdown,
    /// SQLite database with topics + sources + conflicts tables
    Sqlite,
}
```

**ExportFilter** controls what gets exported:
- `All` — everything (respecting privacy, see §2.7)
- `Topics(Vec<TopicId>)` — specific topics
- `Query(String)` — topics matching a search query
- `MinFreshness(f32)` — only topics above a freshness threshold
- `Category(String)` — topics in a category

**Privacy enforcement on export**: The export engine calls into the PrivacyGuard
(§2.7) before writing any topic. Topics with `PrivacyLevel::Private` are excluded
unless the caller has explicit override. Topics with `PrivacyLevel::Sensitive` go
through the redaction pipeline before export.

**ImportPolicy** controls merge behavior:
- `CreateOnly` — skip topics that already exist (by content hash)
- `UpdateIfNewer` — overwrite if imported topic has newer timestamp
- `ForceOverwrite` — always overwrite

### §2.5 CLI Interface

**Satisfies**: GOAL-maint.7

Exposes maintenance and access functionality as CLI subcommands under `engram-ai knowledge`.

```
engram-ai knowledge
├── query <search-term> [--limit N] [--format json|text] [--min-freshness F]
├── inspect <topic-id> [--sources] [--conflicts] [--history]
├── export [--filter EXPR] [--format json|md|sqlite] [--output PATH]
├── import <path> [--policy create|update|force]
├── health [--scope all|stale|conflicted] [--format json|text]
├── decay [--evaluate | --apply] [--topic TOPIC_ID | --all]
├── conflicts [--scan | --resolve CONFLICT_ID --action merge|keep|dismiss]
├── audit [--links | --duplicates] [--repair]
└── privacy [--set-level TOPIC_ID LEVEL] [--audit-log] [--redact-dry-run TOPIC_ID]
```

Each subcommand maps directly to the corresponding component's API. The CLI layer
is thin: argument parsing (via `clap`) → call component API → format output.

**Output formats**: All commands support `--format json` for machine consumption
and `--format text` (default) for human-readable output. The `health` command
additionally supports `--format markdown` for report generation.

### §2.6 Programmatic API

**Satisfies**: GOAL-maint.8

A Rust API (`pub mod maintenance`) that mirrors every CLI capability. The CLI is
implemented as a thin wrapper over this API — ensuring feature parity.

```rust
pub struct MaintenanceApi {
    store: Arc<KnowledgeStore>,
    decay_engine: DecayEngine,
    conflict_detector: ConflictDetector,
    health_auditor: HealthAuditor,
    export_engine: ExportEngine,
    privacy_guard: PrivacyGuard,
}

impl MaintenanceApi {
    // Query & access
    pub fn query(&self, q: &str, opts: QueryOpts) -> Result<Vec<QueryResult>>;
    pub fn inspect(&self, topic_id: TopicId) -> Result<TopicDetail>;

    // Knowledge-aware recall (GOAL-maint.6)
    /// Boost recall results with compiled topic pages.
    /// When a query matches a topic page, the page is included in results
    /// with a configurable weight boost (default: 1.5x) over fragment memories.
    /// Topic pages participate in hybrid search (FTS5 + embedding) alongside
    /// regular memories. The caller (engram recall pipeline) calls this method
    /// and merges results with standard memory recall.
    pub fn recall_with_topics(&self, q: &str, opts: RecallOpts) -> Result<Vec<RecallResult>>;

    // Operation summaries (GOAL-maint.5b)
    /// Returns a summary of the last maintenance/compile operation.
    /// Each mutating operation (compile, decay apply, conflict resolve) produces
    /// an OperationSummary with counts, timing, and LLM token cost.
    pub fn last_operation_summary(&self) -> Option<OperationSummary>;

    // Maintenance operations
    pub fn evaluate_decay(&self, scope: DecayScope) -> Result<Vec<DecayResult>>;
    pub fn apply_decay(&self, topic_id: TopicId, action: DecayAction) -> Result<()>;
    pub fn detect_conflicts(&self, scope: ConflictScope) -> Result<Vec<Conflict>>;
    pub fn resolve_conflict(&self, id: ConflictId, action: ConflictResolution) -> Result<()>;
    pub fn audit_links(&self, scope: AuditScope) -> Result<LinkAuditReport>;
    pub fn repair_link(&self, link_id: LinkId, action: LinkRepairAction) -> Result<()>;
    pub fn health_report(&self, scope: ReportScope) -> Result<HealthReport>;

    // Export/import
    pub fn export(&self, filter: ExportFilter, fmt: ExportFormat, dest: &Path) -> Result<ExportManifest>;
    pub fn import(&self, source: &Path, policy: ImportPolicy) -> Result<ImportReport>;

    // Privacy
    pub fn set_privacy_level(&self, topic_id: TopicId, level: PrivacyLevel) -> Result<()>;
    pub fn redact_preview(&self, topic_id: TopicId) -> Result<RedactedTopic>;
    pub fn access_audit_log(&self, filter: AuditFilter) -> Result<Vec<AuditEntry>>;
}
```

All methods return `Result<T>` with a unified `MaintenanceError` enum. No panics
in the public API surface.

### §2.7 Privacy Guard

**Satisfies**: GOAL-maint.9, GOAL-maint.10, GOAL-maint.11

Enforces visibility, redaction, and audit logging across all access paths.

```
PrivacyGuard
├── check_access(topic_id, context: AccessContext) -> AccessDecision
├── redact(topic: &CompiledTopic) -> RedactedTopic
├── log_access(entry: AuditEntry) -> Result<()>
└── query_audit_log(filter: AuditFilter) -> Result<Vec<AuditEntry>>
```

**Privacy levels** (per-topic, stored in `CompiledTopic` metadata):

```rust
pub enum PrivacyLevel {
    /// Visible in all queries and exports (default)
    Public,
    /// Visible in queries but redacted before export
    Sensitive,
    /// Hidden from queries unless explicitly requested with --include-private
    Private,
}
```

**Redaction pipeline** (for `Sensitive` topics before export):
1. Entity detection — scan compiled text for patterns matching configured
   sensitive entity types (names, emails, API keys, file paths, etc.)
2. Replacement — substitute detected entities with type-tagged placeholders
   (`[PERSON-1]`, `[EMAIL-1]`, `[API_KEY-1]`)
3. Consistency — same entity gets the same placeholder across the topic

```rust
pub struct RedactionConfig {
    /// Entity types to redact
    pub entity_types: Vec<EntityType>,
    /// Custom regex patterns for domain-specific secrets
    pub custom_patterns: Vec<(String, String)>,  // (name, regex)
    /// Whether to redact file paths (default: true for export)
    pub redact_paths: bool,
}

pub enum EntityType {
    PersonName,
    Email,
    ApiKey,
    IpAddress,
    FilePath,
    Custom(String),
}
```

**Audit log**: Every access to a `Private` or `Sensitive` topic is logged:

```rust
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub topic_id: TopicId,
    pub action: AuditAction,
    pub accessor: String,       // CLI user, API caller identifier
    pub privacy_level: PrivacyLevel,
    pub was_redacted: bool,
}

pub enum AuditAction {
    Query,
    Inspect,
    Export,
    PrivacyLevelChange { from: PrivacyLevel, to: PrivacyLevel },
}
```

The audit log is stored in a separate SQLite table (`knowledge_audit_log`),
append-only.

**Dependency note**: Entity-based redaction depends on engram's entity extraction
system (`src/entities.rs`, Aho-Corasick + regex). If entity extraction is not
available (e.g., feature-gated out), redaction falls back to regex-only patterns
(`custom_patterns` in `RedactionConfig`). The fallback covers API keys, emails,
and IP addresses but not person names.

## 3. Multi-Process Safety

**Satisfies**: GOAL-maint.12

Maintenance operations (decay evaluation, conflict scanning, link auditing) must
not run concurrently from multiple processes. The system uses file-based leader
election:

```rust
pub struct MaintenanceLock {
    lock_path: PathBuf,  // e.g., <data_dir>/maintenance.lock
}

impl MaintenanceLock {
    /// Try to acquire exclusive lock. Returns Err if another process holds it.
    pub fn try_acquire(&self) -> Result<MaintenanceGuard>;
    /// Check if lock is held (and by whom — PID written in lock file)
    pub fn status(&self) -> LockStatus;
}

pub enum LockStatus {
    Free,
    Held { pid: u32, since: DateTime<Utc> },
    Stale { pid: u32, since: DateTime<Utc> },  // PID no longer running
}
```

**Stale lock recovery**: If the lock file exists but the PID is not running,
the lock is considered stale and can be forcibly acquired (with a warning log).

**Lock file path**: `{db_directory}/.engram-maintenance.lock`. If the lock path
is not writable (e.g., read-only filesystem), maintenance operations that require
the lock are skipped with a warning: "Cannot acquire maintenance lock: {path} not
writable. Skipping mutating maintenance operations."

The `MaintenanceApi` acquires this lock at the start of any mutating maintenance
operation (decay apply, conflict resolve, link repair) and releases it on
completion. Read-only operations (health report, query, inspect) do not require
the lock.

## 4. Data Flow

```
                    ┌─────────────────────────────┐
                    │     MaintenanceScheduler     │
                    │   (from architecture §3.2)   │
                    └──────────┬──────────────────┘
                               │ triggers
                    ┌──────────▼──────────────────┐
                    │      MaintenanceApi          │
                    │  (orchestrates all below)    │
                    └──┬─────┬──────┬──────┬──────┘
                       │     │      │      │
              ┌────────▼┐ ┌─▼────┐ │  ┌───▼──────┐
              │  Decay   │ │Confl.│ │  │  Health   │
              │  Engine  │ │Detect│ │  │  Auditor  │
              └────┬─────┘ └──┬───┘ │  └────┬─────┘
                   │          │     │       │
                   ▼          ▼     │       ▼
              KnowledgeStore (SQLite)│   HealthReport
                                    │
                          ┌─────────▼────────┐
                          │  Export/Import    │
                          │  Engine           │
                          └────────┬─────────┘
                                   │
                          ┌────────▼─────────┐
                          │  Privacy Guard   │
                          │  (filter+redact) │
                          └────────┬─────────┘
                                   │
                              Output files
                          (JSON / MD / SQLite)
```

External access flows:
- **CLI** → parses args → calls `MaintenanceApi` → formats output → stdout
- **Rust API** → direct `MaintenanceApi` method calls → returns typed `Result<T>`

Both paths go through `PrivacyGuard` for any operation that exposes topic content.

## 5. Integration with Architecture

This feature connects to the shared architecture (§ references to `architecture.md`):

- **KnowledgeStore** (arch §2): All components read/write compiled topics through
  the shared store. Decay scores, conflict records, and privacy levels are stored
  as topic metadata fields.
- **MaintenanceScheduler** (arch §3.2): Triggers `DecayEngine.evaluate_all()`,
  `ConflictDetector.detect_conflicts(RecentlyChanged)`, and
  `HealthAuditor.audit_links(All)` on configurable intervals.
- **MaintenanceTask** (arch §3.2): Each maintenance operation is wrapped as a
  `MaintenanceTask` enum variant, dispatched by the scheduler.
- **EventBus** (arch §3.1): Maintenance operations emit events
  (`TopicDecayed`, `ConflictDetected`, `LinkBroken`, `ExportCompleted`) for
  observability and cross-feature coordination.
- **Shared types** (arch §4): Uses `TopicId`, `CompiledTopic`, `ConfidenceScore`,
  `CompilationMetadata` directly. Extends `CompiledTopic` with `freshness_score`,
  `privacy_level`, and `last_accessed` fields.

## 6. Traceability Matrix

| GOAL | Component(s) | Section |
|------|-------------|---------|
| GOAL-maint.1 | Decay Engine | §2.1 |
| GOAL-maint.2 | Conflict Detector | §2.2 |
| GOAL-maint.3 | Link & Health Auditor | §2.3 |
| GOAL-maint.4 | Conflict Detector | §2.2 |
| GOAL-maint.5 | Link & Health Auditor | §2.3 |
| GOAL-maint.5b | MaintenanceApi (operation summaries) | §2.6 |
| GOAL-maint.6 | MaintenanceApi (knowledge-aware recall) | §2.6 |
| GOAL-maint.7 | CLI Interface | §2.5 |
| GOAL-maint.8 | Programmatic API | §2.6 |
| GOAL-maint.9 | Privacy Guard | §2.7 |
| GOAL-maint.10 | Privacy Guard | §2.7 |
| GOAL-maint.11 | Privacy Guard | §2.7 |
| GOAL-maint.12 | Multi-Process Safety | §3 |

All 13 GOALs (including maint.5b) are covered. No orphan components (every component satisfies ≥1 GOAL).

**GUARD compliance notes:**
- GUARD-1 (Engram-Native): All maintenance code in `src/compiler/maintenance/`. No external dependencies.
- GUARD-2 (Incremental): ConflictScope.RecentlyChanged limits scan to changed topics.
- GUARD-3 (LLM Cost): ConflictDetector budget-gates LLM calls. OperationSummary tracks total token cost.
- GUARD-5 (Non-Destructive): All mutations go through MaintenanceLock. Decay/archival preserves data.
- GUARD-6 (Offline-First): Health reports, decay evaluation, link audit all work without LLM. Only ConflictDetector uses LLM (degrades gracefully).
