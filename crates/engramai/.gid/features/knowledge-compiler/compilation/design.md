# Design: Topic Compilation & Feedback

> Feature-level design for the compilation pipeline: topic discovery, compilation,
> incremental recompilation, merge/split lifecycle, and user feedback.
> Parent architecture: `/Users/potato/clawd/projects/engram-ai-rust/.gid/docs/architecture.md`

## §1 Overview

### 1.1 Purpose

Transform raw engram memories into structured, navigable topic pages through an automated pipeline that discovers topics via clustering, compiles them with LLM synthesis, recompiles incrementally when source memories change, manages topic lifecycle (merge/split), and incorporates user feedback to improve quality over time.

### 1.2 Goals and Non-Goals

**Goals:**
- Compile memories into navigable topic pages with provenance (GOAL-comp.1, GOAL-comp.3)
- Fully automated topic discovery from memory clusters (GOAL-comp.2)
- Incremental recompilation that avoids redundant LLM calls (GOAL-comp.3)
- Topic lifecycle management: merge, split, cross-linking (GOAL-comp.4, GOAL-comp.5, GOAL-comp.6)
- Manual editing preserved across recompilation (GOAL-comp.7)
- User feedback loop with preview capability (GOAL-comp.8, GOAL-comp.9)
- Graceful failure handling preserving previous versions (GOAL-comp.10)

**Non-Goals:**
- Real-time streaming compilation (batch is sufficient)
- Multi-user collaborative editing of topic pages
- Custom LLM fine-tuning from feedback (we use prompt engineering only)
- Visual/graphical topic map rendering (text output only)

### 1.3 Component Summary (6 components, ≤8 limit)

| ID | Component | Primary GOALs |
|----|-----------|---------------|
| §3.1 | TopicDiscovery | GOAL-comp.2 |
| §3.2 | CompilationPipeline | GOAL-comp.1, GOAL-comp.3, GOAL-comp.7, GOAL-comp.9, GOAL-comp.10 |
| §3.3 | IncrementalTrigger | GOAL-comp.3 |
| §3.4 | TopicLifecycle | GOAL-comp.4, GOAL-comp.5, GOAL-comp.6 |
| §3.5 | FeedbackProcessor | GOAL-comp.8 |
| §3.6 | QualityScorer | (quality metrics — supports comp.5 split detection, comp.10 reporting) |

## §2 Shared Types

These types are defined in the architecture doc (§4) and used across this feature:

```rust
// From architecture §4 — used here, not redefined
use crate::kc::{
    TopicPage, TopicId, CompilationRecord, SourceMemoryRef,
    KcConfig, RecompileStrategy, FeedbackEntry, FeedbackKind,
    HealthReport, TopicStatus,
};
```

### 2.1 Feature-Local Types

```rust
/// Result of topic discovery: a candidate cluster ready for compilation.
pub struct TopicCandidate {
    /// Suggested topic label (from LLM or heuristic)
    pub label: String,
    /// Memory IDs that form this topic
    pub memory_ids: Vec<MemoryId>,
    /// Clustering confidence score [0.0, 1.0]
    pub confidence: f64,
    /// Whether this overlaps with an existing TopicPage
    pub overlaps_with: Option<TopicId>,
}

/// Tracks what changed since last compilation for incremental decisions.
pub struct ChangeSet {
    /// Memories added since last compile
    pub added: Vec<MemoryId>,
    /// Memories whose content was modified
    pub modified: Vec<MemoryId>,
    /// Memories that were deleted/decayed
    pub removed: Vec<MemoryId>,
    /// Timestamp of the last compilation
    pub last_compiled: DateTime<Utc>,
}

/// Outcome of an incremental trigger evaluation.
pub enum TriggerDecision {
    /// No recompilation needed
    Skip { reason: String },
    /// Recompile only the changed sections
    Partial { affected_sections: Vec<String>, change_set: ChangeSet },
    /// Full recompilation required
    Full { reason: String, change_set: ChangeSet },
}

/// A merge/split operation on topics.
pub enum LifecycleOp {
    Merge {
        sources: Vec<TopicId>,
        target_label: String,
    },
    Split {
        source: TopicId,
        new_clusters: Vec<Vec<MemoryId>>,
    },
}

/// User feedback attached to a specific topic page.
pub struct TopicFeedback {
    pub topic_id: TopicId,
    pub kind: FeedbackKind,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    /// Which section of the topic page (if applicable)
    pub section: Option<String>,
}

/// Quality assessment of a compiled topic page.
pub struct QualityReport {
    pub topic_id: TopicId,
    /// Overall score [0.0, 1.0]
    pub score: f64,
    /// Breakdown by dimension
    pub coherence: f64,
    pub coverage: f64,
    pub freshness: f64,
    /// Actionable suggestions
    pub suggestions: Vec<String>,
}
```

## §3 Component Designs

### §3.1 TopicDiscovery

**Traces:** GOAL-comp.2 (automatic topic discovery from memory clusters)

**Purpose:** Discover topic candidates by clustering memories using their existing embeddings from engram's `memory_embeddings` table. Label candidates via LLM when available.

**Architectural principle — embedding reuse:** TopicDiscovery does NOT generate its own embeddings for source memories. It reads pre-computed embeddings from engram's `memory_embeddings` table (via `Storage::get_all_embeddings(model)`). This is the same embedding used by `hybrid_search` and `find_nearest_embedding`. When a memory lacks an embedding (provider was offline at store time), the caller provides a hash-based fallback — but this is degraded mode, not the design intent.

**Design:**

```rust
pub struct TopicDiscovery {
    min_cluster_size: usize,
    overlap_threshold: f64,
}

impl TopicDiscovery {
    /// Discover topic candidates from memories with pre-computed embeddings.
    /// Embeddings should come from engram's memory_embeddings table.
    /// The caller is responsible for loading embeddings and falling back
    /// to hash-based pseudo-embeddings for memories without stored vectors.
    pub fn discover(
        &self,
        memories: &[(String, Vec<f32>)], // (memory_id, embedding from memory_embeddings)
    ) -> Vec<TopicCandidate> { .. }
}
```

**Algorithm:**

1. Caller loads embeddings from `Storage::get_all_embeddings(model_id)` and builds `(memory_id, embedding)` pairs. For memories without stored embeddings, caller uses `simple_hash_embedding()` as fallback.
2. `discover()` runs agglomerative clustering (single-linkage, similarity threshold 0.5) on the provided embeddings.
3. For each cluster above `min_cluster_size`, build a `TopicCandidate`:
   - Compute `centroid_embedding` as the mean of member embeddings.
   - Compute `cohesion_score` from average intra-cluster cosine similarity.
   - `suggested_title` is `None` until `label_cluster()` is called.
4. Optionally call `label_cluster()` — sends the cluster's memory contents to LLM with prompt: "Given these related memories, suggest a concise topic label (2-5 words)."
5. Run `detect_overlap()` against existing `TopicPage` list — if Jaccard similarity of memory sets exceeds threshold, set `overlaps_with`.
6. Return sorted by cohesion descending.

**Edge cases:**
- Memories belonging to multiple clusters: each cluster gets its own candidate; the `TopicLifecycle` component (§3.4) handles merge decisions later.
- Zero clusters found: return empty vec, caller logs "no topics discovered."
- LLM labeling failure: fall back to entity-frequency heuristic (most common entity in the cluster becomes the label).

---

### §3.2 CompilationPipeline

**Traces:** GOAL-comp.1 (compile memories into topic pages), GOAL-comp.3 (section-level provenance/attribution), GOAL-comp.7 (preserve manual edits), GOAL-comp.9 (dry-run preview), GOAL-comp.10 (failure handling)

**Purpose:** Given a `TopicCandidate` (or an existing `TopicPage` for recompilation), produce a compiled topic page with full provenance tracking — every paragraph traces back to source memories.

**Design:**

```rust
pub struct CompilationPipeline {
    config: KcConfig,
    llm: Arc<dyn SynthesisLlmProvider>,
}

impl CompilationPipeline {
    /// Compile a new topic page from a candidate.
    pub async fn compile_new(
        &self,
        candidate: &TopicCandidate,
        memories: &[Memory],
    ) -> Result<(TopicPage, CompilationRecord), KcError> { .. }

    /// Recompile an existing topic page (full recompilation).
    pub async fn recompile_full(
        &self,
        topic: &TopicPage,
        memories: &[Memory],
    ) -> Result<(TopicPage, CompilationRecord), KcError> { .. }

    /// Recompile only affected sections (partial).
    pub async fn recompile_partial(
        &self,
        topic: &TopicPage,
        affected_sections: &[String],
        change_set: &ChangeSet,
        memories: &[Memory],
    ) -> Result<(TopicPage, CompilationRecord), KcError> { .. }
}
```

**Compilation flow (compile_new):**

1. Gather all memories referenced by `candidate.memory_ids`.
2. Sort memories by timestamp (chronological narrative).
3. Build compilation prompt:
   ```
   You are synthesizing a knowledge topic page.
   Topic: "{candidate.label}"

   Source memories (each has an ID):
   [MEM-001]: {content}
   [MEM-002]: {content}
   ...

   Instructions:
   - Organize into coherent sections
   - After each paragraph, cite source memory IDs in brackets: [MEM-001, MEM-003]
   - Resolve contradictions by preferring the more recent memory
   - Note any gaps or uncertainties
   ```
4. Send to LLM via `SynthesisLlmProvider::generate()`.
5. Parse LLM output into `TopicPage`:
   - Extract sections and their citation markers.
   - Build `SourceMemoryRef` entries for each cited memory.
   - Populate `CompilationRecord` with input memory IDs, output hash, timestamp, LLM model used.
6. Validate: every section must have at least one citation. If any section has zero → re-prompt that section with explicit instruction to cite.

**Provenance guarantee (GOAL-comp.3):**
- The prompt enforces `[MEM-XXX]` citation syntax.
- Post-processing parses citations and builds a `provenance: Vec<ProvenanceRecord>` on the `TopicPage`.
- Each `ProvenanceRecord` links (section_index, paragraph_index) → Vec<MemoryId>.
- If citation parsing fails (LLM didn't follow format), fall back to embedding similarity: for each paragraph, find top-3 most similar source memories and attach as provenance.

**Partial recompilation (recompile_partial):**
- Only re-prompts sections whose source memories appear in `change_set.added` or `change_set.modified`.
- Unchanged sections are preserved verbatim.
- The `CompilationRecord` tracks which sections were recompiled vs preserved.

**Manual edit preservation (GOAL-comp.7):**
- Each section in a `TopicPage` has a `user_edited: bool` flag.
- When a user edits a section via CLI/API, that section is marked `user_edited = true`.
- During `recompile_partial()` and `recompile_full()`:
  - User-edited sections are **never overwritten**. They are passed to the LLM as "fixed sections" in the prompt.
  - New content from source memories is added around user-edited sections.
  - If new source material contradicts a user edit, a `ConflictFlag` is attached to the section (surfaced to maintenance/conflict detection, not silently resolved).
- Prompt addendum for user-edited sections:
  ```
  FIXED SECTIONS (do not modify these — user has edited them):
  - Section "Overview": {user's version}
  
  Add new content around fixed sections. If new source material
  contradicts a fixed section, note the conflict explicitly.
  ```

**Compilation dry-run (GOAL-comp.9):**

```rust
impl CompilationPipeline {
    /// Preview what would change without executing.
    pub async fn dry_run(
        &self,
        topics: &[TopicPage],
        trigger: &IncrementalTrigger,
        memories: &[Memory],
    ) -> Result<DryRunReport, KcError> { .. }
}

pub struct DryRunReport {
    /// Topics that would be compiled/recompiled
    pub affected_topics: Vec<DryRunEntry>,
    /// Total estimated token count (input + output)
    pub estimated_tokens: u64,
    /// Estimated cost in USD (based on model pricing)
    pub estimated_cost_usd: f64,
    /// Whether cost exceeds user-budget threshold
    pub exceeds_budget: bool,
}

pub struct DryRunEntry {
    pub topic_id: TopicId,
    pub action: DryRunAction,  // New, PartialRecompile, FullRecompile
    pub estimated_tokens: u64,
    pub reason: String,
}
```

- `dry_run()` evaluates `IncrementalTrigger` for each topic, collects `TriggerDecision`s.
- For each non-Skip decision, estimates token count from source memory lengths + prompt template size.
- Cost estimated from `KcConfig::model_pricing` (tokens × price-per-token).
- If `estimated_cost_usd > config.budget_threshold`, sets `exceeds_budget = true` — the caller (CLI/API) requires explicit user confirmation before proceeding.
- No data is modified during dry-run.

**Failure handling (GOAL-comp.10):**

```rust
pub struct CompilationError {
    pub topic_id: TopicId,
    pub error_kind: CompilationErrorKind,
    pub message: String,
    /// LLM tokens consumed before failure (for cost tracking)
    pub tokens_consumed: u64,
}

pub enum CompilationErrorKind {
    LlmUnavailable,
    UnparseableOutput,
    DatabaseWriteFailure,
    ProviderRateLimited,
    BudgetExceeded,
}
```

- When `compile_new()` / `recompile_*()` fails:
  1. **Previous version preserved** — the existing `TopicPage` in storage is untouched (writes are atomic: only committed on full success).
  2. **Page marked stale** — `TopicStatus::Stale { error: Some(CompilationError) }` so next cycle retries.
  3. **Error logged** with `CompilationError` including `tokens_consumed` for cost tracking even on failure.
  4. **Batch continues** — the batch orchestrator catches `Err(CompilationError)` per topic and proceeds to the next topic. Successfully compiled pages are committed independently.
- Retry policy: failed topics are retried in the next compilation cycle. After `config.max_retries` (default: 3) consecutive failures, topic is marked `TopicStatus::FailedPermanent` and excluded from automatic compilation until manual intervention.

---

### §3.3 IncrementalTrigger

**Traces:** GOAL-comp.3 (incremental compilation — detect stale pages, recompile only affected)

**Purpose:** Evaluate whether a topic needs recompilation based on changes to its source memories, and decide between skip/partial/full recompilation.

**Design:**

```rust
pub struct IncrementalTrigger {
    config: KcConfig,
}

impl IncrementalTrigger {
    /// Evaluate whether a topic needs recompilation.
    pub fn evaluate(
        &self,
        topic: &TopicPage,
        current_memories: &[Memory],
    ) -> TriggerDecision { .. }

    /// Build a ChangeSet by diffing current memories against last compilation.
    fn compute_change_set(
        &self,
        topic: &TopicPage,
        current_memories: &[Memory],
    ) -> ChangeSet { .. }
}
```

**Decision logic:**

```
let cs = compute_change_set(topic, current_memories);
let total_changes = cs.added.len() + cs.modified.len() + cs.removed.len();
let total_sources = topic.source_memory_ids.len();

match config.recompile_strategy {
    RecompileStrategy::Eager => {
        if total_changes == 0 { Skip }
        else if total_changes as f64 / total_sources as f64 > 0.5 { Full }
        else { Partial }
    }
    RecompileStrategy::Lazy => {
        if total_changes as f64 / total_sources as f64 > 0.3 { Full }
        else if total_changes > 0 { Partial }
        else { Skip }
    }
    RecompileStrategy::Manual => Skip  // only recompile on explicit request
}
```

**Redundancy avoidance (GOAL-comp.5):**
- `compute_change_set` compares `topic.compilation_record.input_memory_hashes` against current memory content hashes.
- A memory that was "modified" but whose content hash is identical → excluded from change set (cosmetic edit, no semantic change).
- If `ChangeSet` is all empty → `TriggerDecision::Skip` with reason.
- The caller (orchestrator) caches `TriggerDecision` results per topic to avoid re-evaluating within the same batch run.

**Partial vs Full threshold:**
- `> 50%` of source memories changed → Full (cheaper to redo than patch)
- `≤ 50%` changed → Partial (only affected sections)
- Configurable via `KcConfig` thresholds (default: 0.5 for eager, 0.3 for lazy)

---

### §3.4 TopicLifecycle

**Traces:** GOAL-comp.4 (merge overlapping topics), GOAL-comp.5 (split oversized topics), GOAL-comp.6 (cross-topic linking)

**Purpose:** Manage topic evolution over time — detecting when topics should merge (high overlap) or split (low coherence), and executing these operations while preserving provenance.

**Design:**

```rust
pub struct TopicLifecycle {
    config: KcConfig,
    llm: Arc<dyn SynthesisLlmProvider>,
}

impl TopicLifecycle {
    /// Analyze all topics and suggest lifecycle operations.
    pub async fn analyze(&self, topics: &[TopicPage]) -> Vec<LifecycleOp> { .. }

    /// Execute a merge operation.
    pub async fn execute_merge(
        &self,
        op: &LifecycleOp,
        pipeline: &CompilationPipeline,
        memories: &[Memory],
    ) -> Result<TopicPage, KcError> { .. }

    /// Execute a split operation.
    pub async fn execute_split(
        &self,
        op: &LifecycleOp,
        pipeline: &CompilationPipeline,
        memories: &[Memory],
    ) -> Result<Vec<TopicPage>, KcError> { .. }
}
```

**Merge detection (GOAL-comp.6):**
1. For each pair of topics, compute memory overlap ratio: `|A ∩ B| / min(|A|, |B|)`.
2. If overlap > 0.6 → suggest `LifecycleOp::Merge`.
3. Merge execution:
   - Union all source memory IDs from both topics.
   - Ask LLM for best combined label.
   - Call `CompilationPipeline::compile_new()` with the unified memory set.
   - Mark original topics as `TopicStatus::Merged { into: new_topic_id }`.
   - Provenance chain: new topic's `CompilationRecord` references both original topic IDs.

**Split detection (GOAL-comp.7):**
1. For each topic with > `2 * config.min_topic_size` memories, re-run clustering on its source memories.
2. If clustering produces 2+ distinct clusters with confidence > 0.7 → suggest `LifecycleOp::Split`.
3. Split execution:
   - For each sub-cluster, call `CompilationPipeline::compile_new()`.
   - Mark original topic as `TopicStatus::Split { into: vec![new_ids] }`.
   - Each new topic inherits relevant provenance from the original.

**Cross-topic linking (GOAL-comp.6):**
1. After any compilation (new or recompile), compute links to other topics:
   - **Shared memories**: `|A ∩ B| / min(|A|, |B|)` — Jaccard similarity of source memory sets.
   - **Shared entities**: entity overlap between topic page texts.
   - **Embedding similarity**: cosine similarity between topic page embeddings.
2. Link strength = weighted combination: `0.5 * shared_memories + 0.3 * shared_entities + 0.2 * embedding_sim`.
3. Links with strength ≥ 0.3 are stored. Links categorized by type:
   - `Related` (shared memories but different angles)
   - `SubTopic` (one topic is subset of another)
   - `Prerequisite` (temporal ordering — earlier topic provides context for later)
4. Cross-topic links are recomputed when any linked topic is recompiled.

```rust
pub struct CrossTopicLink {
    pub source: TopicId,
    pub target: TopicId,
    pub link_type: LinkType,
    pub strength: f64,  // [0.0, 1.0]
    pub reason: String,  // e.g. "shares 5 source memories"
}

pub enum LinkType {
    Related,
    SubTopic,
    Prerequisite,
}
```

**Provenance preservation:**
- Merge/split never destroys provenance. Original `CompilationRecord` entries are retained.
- New topics link back to originals via `TopicStatus` enum, creating an auditable history chain.

---

### §3.5 FeedbackProcessor

**Traces:** GOAL-comp.8 (point-level feedback: ✅ correct, ❌ wrong, 🔄 outdated)

**Purpose:** Process user feedback (corrections, ratings, suggested edits) and integrate it into the compilation loop so future recompilations reflect user intent.

**Design:**

```rust
pub struct FeedbackProcessor {
    config: KcConfig,
}

impl FeedbackProcessor {
    /// Record feedback for a topic.
    pub fn record(&self, feedback: TopicFeedback, store: &mut impl FeedbackStore) -> Result<(), KcError> { .. }

    /// Build a feedback context for inclusion in compilation prompts.
    pub fn build_prompt_context(
        &self,
        topic_id: &TopicId,
        store: &impl FeedbackStore,
    ) -> String { .. }

    /// Determine if feedback warrants immediate recompilation.
    pub fn should_trigger_recompile(&self, feedback: &TopicFeedback) -> bool { .. }
}

/// Storage trait for feedback entries.
pub trait FeedbackStore {
    fn save(&mut self, entry: FeedbackEntry) -> Result<(), KcError>;
    fn get_for_topic(&self, topic_id: &TopicId) -> Vec<FeedbackEntry>;
}
```

**Feedback flow (GOAL-comp.8):**
1. User submits `TopicFeedback` via CLI or API with one of: `✅ Correct`, `❌ Wrong`, `🔄 Outdated`.
2. `record()` persists as `FeedbackEntry` in the engram DB (table: `kc_feedback`).
3. `should_trigger_recompile()` checks:
   - `FeedbackKind::Wrong` → true (marked for removal/regeneration on next compile)
   - `FeedbackKind::Outdated` → true (triggers recompilation of that section)
   - `FeedbackKind::Correct` → false (informational — marks key point as user-validated, preserved in future compiles)

**Integration into recompilation:**
1. When `CompilationPipeline` recompiles a topic, it calls `build_prompt_context()`.
2. This method retrieves all unresolved feedback for the topic and formats it as prompt addendum:
   ```
   User feedback to incorporate:
   - [CORRECTION]: "The date should be 2024, not 2023" (section: "Timeline")
   - [EDIT]: "Add mention of the Rust rewrite decision" (section: "History")
   ```
3. The compilation prompt includes this context, instructing the LLM to incorporate corrections and consider suggestions.
4. After successful recompilation, feedback entries are marked as `resolved: true` with the compilation record ID that addressed them.

---

### §3.6 QualityScorer

**Traces:** Quality metrics for compiled topics (supports GOAL-comp.5 split detection, GOAL-comp.10 reporting)

**Purpose:** Assess the quality of compiled topic pages across multiple dimensions, producing actionable scores that inform recompilation priority and surface low-quality topics for review.

**Design:**

```rust
pub struct QualityScorer {
    config: KcConfig,
}

impl QualityScorer {
    /// Score a compiled topic page.
    pub fn score(&self, topic: &TopicPage, memories: &[Memory], feedback: &[FeedbackEntry]) -> QualityReport { .. }

    /// Rank all topics by quality, worst first.
    pub fn rank_topics(&self, reports: &[QualityReport]) -> Vec<&QualityReport> { .. }
}
```

**Scoring dimensions:**

1. **Coherence** (0.0–1.0): Measures internal consistency.
   - Computed via embedding similarity between adjacent sections.
   - Low similarity between consecutive sections → low coherence.
   - Formula: `avg(cosine_sim(section[i], section[i+1]))` for all adjacent pairs.

2. **Coverage** (0.0–1.0): Measures how much of the source material is represented.
   - `cited_memories / total_source_memories`.
   - Memories not cited in any section → coverage gap.

3. **Freshness** (0.0–1.0): Measures temporal relevance.
   - `1.0 - (days_since_last_compile / max_staleness_days)`, clamped to [0, 1].
   - `max_staleness_days` from `KcConfig` (default: 30).

4. **Overall score**: Weighted average.
   - `0.4 * coherence + 0.35 * coverage + 0.25 * freshness`
   - Adjusted by feedback: each unresolved `ThumbsDown` reduces score by 0.05 (capped at -0.2).

**Suggestions generation:**
- Coverage < 0.7 → "N source memories are uncited — consider recompilation"
- Freshness < 0.3 → "Topic hasn't been recompiled in N days"
- Coherence < 0.5 → "Topic sections may need reorganization — consider split"
- Unresolved corrections > 0 → "N user corrections pending — recompile to incorporate"

## §4 Data Flow

```
                    ┌─────────────────┐
                    │  Memory Store   │
                    │  (engram DB)    │
                    └────────┬────────┘
                             │ memories
                             ▼
                    ┌─────────────────┐
                    │ TopicDiscovery  │──── GOAL-comp.1
                    │   (§3.1)       │
                    └────────┬────────┘
                             │ TopicCandidate[]
                             ▼
                    ┌─────────────────┐
                    │ CompilationPipe │──── GOAL-comp.2, comp.3
                    │   (§3.2)       │◄─── prompt context from FeedbackProcessor
                    └────────┬────────┘
                             │ TopicPage + CompilationRecord
                             ▼
              ┌──────────────┼──────────────┐
              ▼              ▼              ▼
    ┌──────────────┐ ┌─────────────┐ ┌──────────────┐
    │ Incremental  │ │  Topic      │ │  Quality     │
    │ Trigger      │ │  Lifecycle  │ │  Scorer      │
    │  (§3.3)      │ │  (§3.4)    │ │  (§3.6)      │
    │ GOAL-comp.4,5│ │ GOAL-comp. │ │ GOAL-comp.10 │
    └──────┬───────┘ │   6, 7     │ └──────────────┘
           │         └─────┬──────┘
           │ TriggerDecision│ LifecycleOp
           └───────┬────────┘
                   ▼
          ┌─────────────────┐
          │ CompilationPipe │  (recompile_full or recompile_partial)
          │   (§3.2)       │
          └─────────────────┘
                   ▲
                   │ prompt context
          ┌─────────────────┐
          │ FeedbackProc    │──── GOAL-comp.8, comp.9
          │   (§3.5)       │
          └─────────────────┘
                   ▲
                   │ TopicFeedback
              [User / CLI / API]
```

## §5 GUARD Compliance

| GUARD | How Satisfied |
|-------|---------------|
| GUARD-1 (Engram-Native) | All compilation code lives in `src/compiler/`. TopicDiscovery reuses `synthesis::cluster::discover_clusters()`. No external services beyond LLM API. |
| GUARD-2 (Incremental, Not Batch) | §3.3 IncrementalTrigger evaluates per-topic staleness. Only stale pages are recompiled. Batch mode exists for orchestration but each topic is evaluated independently. |
| GUARD-3 (LLM Cost Awareness) | `CompilationPipeline` tracks tokens per compilation in `CompilationRecord.tokens_used`. `dry_run()` estimates cost before execution. `KcConfig.budget_threshold` gates expensive compilations. Failed compilations record `tokens_consumed` in `CompilationError`. `ModelRouter::for_task()` selects cheapest capable model per operation type. |
| GUARD-4 (Provenance Traceability) | §3.2 enforces citation syntax in LLM prompts. Post-processing builds `ProvenanceRecord` linking (section, paragraph) → Vec<MemoryId>. Fallback: embedding similarity matching if citation parsing fails. |
| GUARD-5 (Non-Destructive) | Compilation never mutates source memories. `compile_new()` and `recompile_*()` only write to `TopicPage` storage. Source memories are read-only inputs. Failure handling preserves previous versions (atomic writes). |
| GUARD-6 (Offline-First) | Compiled topic pages are stored locally and queryable without LLM. Only compilation/recompilation requires LLM. Reading, searching, and browsing compiled pages works fully offline. |

## §6 Integration with Existing Code

| Symbol | Location | Status |
|--------|----------|--------|
| `discover_clusters()` | `src/synthesis/cluster.rs` | ✅ Exists — reused by TopicDiscovery |
| `ClusterDiscoveryConfig` | `src/synthesis/cluster.rs` | ✅ Exists — KC uses with its own thresholds |
| `MemoryCluster` | `src/synthesis/cluster.rs` | ✅ Exists — output of discover_clusters() |
| `SynthesisLlmProvider` | `src/synthesis/mod.rs` | ✅ Exists — will be **replaced** by platform's `LlmProvider` trait (§2.1 of platform design). TopicDiscovery and CompilationPipeline will use `ModelRouter::for_task()` which returns `&dyn LlmProvider`. |
| `Memory`, `MemoryId` | `src/storage.rs` | ✅ Exists — core types |
| `TopicPage`, `TopicId`, `CompilationRecord` | `src/compiler/types.rs` | ⚠️ **New** — to be created |
| `KcConfig` | `src/compiler/config.rs` | ⚠️ **New** — to be created |
| `FeedbackEntry`, `FeedbackStore` | `src/compiler/feedback.rs` | ⚠️ **New** — to be created |
| `CrossTopicLink`, `LinkType` | `src/compiler/lifecycle.rs` | ⚠️ **New** — to be created |

## §7 Trade-offs and Alternatives

### 5.1 LLM-based labeling vs pure heuristic

**Chosen:** LLM labeling with heuristic fallback.
**Alternative:** Pure entity-frequency labeling (no LLM call).
**Trade-off:** LLM produces more natural, descriptive labels but costs an API call per cluster. The heuristic fallback ensures graceful degradation if LLM is unavailable.

### 5.2 Partial vs always-full recompilation

**Chosen:** Hybrid (partial when < 50% changed, full otherwise).
**Alternative:** Always full recompilation (simpler, no section tracking).
**Trade-off:** Partial saves LLM tokens (~60% in typical incremental updates) but requires section-level provenance tracking. The complexity is justified by GOAL-comp.5 (minimize redundant calls).

### 5.3 Feedback as prompt context vs fine-tuning

**Chosen:** Feedback injected as prompt context during recompilation.
**Alternative:** Fine-tune LLM on user-corrected outputs.
**Trade-off:** Prompt injection is zero-cost, immediate, and model-agnostic. Fine-tuning would require training infrastructure and locks us into a specific model. Prompt context is sufficient for the correction volume we expect.

### 5.4 Quality scoring: heuristic vs LLM-as-judge

**Chosen:** Heuristic scoring (embedding similarity + coverage ratio + staleness).
**Alternative:** LLM evaluates quality (more nuanced but expensive).
**Trade-off:** Heuristic is free, deterministic, and fast. LLM-as-judge could be added later as an optional "deep quality audit" mode, but the heuristic covers the common cases well.

## §8 GOAL Traceability Matrix

| GOAL | Component | How Satisfied |
|------|-----------|---------------|
| GOAL-comp.1 | §3.2 CompilationPipeline | compile_new() / recompile_full() produces TopicPage with provenance |
| GOAL-comp.2 | §3.1 TopicDiscovery | Clustering via discover_clusters() + LLM labeling |
| GOAL-comp.3 | §3.3 IncrementalTrigger + §3.2 | ChangeSet diffing triggers recompile; content-hash dedup avoids redundant calls |
| GOAL-comp.4 | §3.4 TopicLifecycle | Jaccard overlap detection + execute_merge() |
| GOAL-comp.5 | §3.4 TopicLifecycle | Re-clustering + execute_split() when >15 key points |
| GOAL-comp.6 | §3.4 TopicLifecycle | Cross-topic linking via shared memories, entities, embedding similarity |
| GOAL-comp.7 | §3.2 CompilationPipeline | user_edited flag per section, preserved during recompile, conflicts flagged |
| GOAL-comp.8 | §3.5 FeedbackProcessor | Point-level feedback (✅/❌/🔄), stored in kc_feedback table |
| GOAL-comp.9 | §3.2 CompilationPipeline | dry_run() estimates cost + shows affected topics before execution |
| GOAL-comp.10 | §3.2 CompilationPipeline | Atomic writes, previous version preserved, error logged with token cost, batch continues |
