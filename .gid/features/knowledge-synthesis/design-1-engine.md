# Design: Knowledge Synthesis — Part 1: Synthesis Engine

**Version**: 1.0  
**Status**: Draft  
**Date**: 2026-04-13  
**Requirements**: requirements-1-engine.md (GOAL-1 through GOAL-4)

---

## §1 Overview

The synthesis engine is the core computation layer of knowledge synthesis. It implements a 4-step pipeline: cluster discovery → gate check → insight generation → provenance tracing. The engine operates on memories already stored in engram's SQLite database, using existing tables (hebbian_links, memory_entities, embeddings) as input signals.

Design principles:
- Zero-LLM for clustering and gate decisions (GUARD-2)
- LLM used only for insight text generation (GOAL-2)
- Every operation is reversible; source memories never deleted (GUARD-1: No Data Loss)
- Insights stored as regular MemoryRecords, distinguishable via metadata (GUARD-5: Insight Identity)
- Additive only — existing consolidate() unchanged (GUARD-3: Backward Compatibility, see master requirements.md)
- Gate uses only existing infrastructure — no new external dependencies (GUARD-4: No New External Dependencies)

---

## §2 Cluster Discovery (GOAL-1)

### §2.1 Signal Sources

Four zero-LLM signals feed cluster discovery:

**Signal 1: Hebbian Links**
- Source: `hebbian_links` table (columns: source_id, target_id, weight, coactivation_timestamp)
- Memories connected by hebbian links with weight ≥ threshold are candidates
- Weight range: 1.0–10.0 (per existing schema)
- Query: SELECT source_id, target_id, weight FROM hebbian_links WHERE weight >= ?

**Signal 2: Entity Overlap**
- Source: `memory_entities` table (columns: memory_id, entity_id, entity_type)
- Memories sharing entities are related
- Query via `get_entity_memories(entity_id)` → returns Vec<String> of memory IDs
- Two memories sharing ≥1 entity get an entity overlap score

**Signal 3: Embedding Similarity**
- Source: embeddings table, accessed via `find_nearest_embedding()`
- Cosine similarity above threshold indicates semantic relatedness
- Uses existing `get_all_embeddings(model)` for batch operations

**Signal 4: Temporal-Activation**
- Memories created or accessed within a time window are more likely related
- Uses MemoryRecord.created_at, last_accessed, access_count
- Recency boosts cluster relevance (newer memories more likely to benefit from synthesis)
- **Design extension**: temporal-activation signal is not in requirements but improves cluster quality by detecting temporal co-occurrence patterns. Recommended for addition to GOAL-1 acceptance criteria.

### §2.2 Multi-Signal Scoring

Each memory pair (i, j) gets a composite relatedness score:

```rust
/// Weights for combining clustering signals
pub struct ClusterWeights {
    pub hebbian: f64,      // default: 0.4
    pub entity: f64,       // default: 0.3
    pub embedding: f64,    // default: 0.2
    pub temporal: f64,     // default: 0.1
}

/// Raw signal scores between two memories
pub struct PairwiseSignals {
    pub hebbian_weight: Option<f64>,    // from hebbian_links table, None if no link
    pub entity_overlap: f64,            // |shared_entities| / |union_entities|, Jaccard
    pub embedding_similarity: f64,     // cosine similarity, 0.0–1.0
    pub temporal_proximity: f64,        // decay function of time gap, 0.0–1.0
}
```

Composite score = weighted sum of normalized signals:
- hebbian: normalized to 0.0–1.0 by dividing by max_weight (10.0)
- entity_overlap: already 0.0–1.0 (Jaccard index)
- embedding_similarity: already 0.0–1.0 (cosine)
- temporal_proximity: exp(-λ × hours_apart), λ from config
  - Half-life based decay: λ = ln(2) / half_life_hours
  - Default half-life: 7 days (168 hours), giving λ ≈ 0.00413
  - At 7 days apart, temporal_proximity ≈ 0.5; at 14 days, ≈ 0.25

### §2.3 Clustering Algorithm

**Algorithm: Connected-components with greedy expansion**

Why connected-components with greedy expansion over k-means or hierarchical agglomerative:
- No need to specify k (number of clusters) upfront
- Works naturally with pairwise scores
- Greedy expansion finds chains of related memories (matches how associations work)
- Computationally feasible for typical memory counts (hundreds to low thousands)

```rust
/// A discovered cluster of related memories
pub struct MemoryCluster {
    pub id: String,                     // deterministic: hash of sorted member IDs
    pub members: Vec<String>,           // memory IDs, sorted
    pub quality_score: f64,             // average intra-cluster relatedness
    pub centroid_id: String,            // member with highest avg relatedness to others
    pub signals_summary: SignalsSummary, // which signals contributed most
}

pub struct SignalsSummary {
    pub dominant_signal: ClusterSignal,
    pub hebbian_contribution: f64,
    pub entity_contribution: f64,
    pub embedding_contribution: f64,
    pub temporal_contribution: f64,
}

pub enum ClusterSignal {
    Hebbian,
    Entity,
    Embedding,
    Temporal,
}
```

**Process:**
1. Compute pairwise scores for all candidate memories (those with ≥1 hebbian link OR accessed in consolidation window)
2. Build edges where score ≥ `cluster_threshold` (configurable, default: 0.3)
3. Apply connected components to find initial clusters
4. For clusters larger than `max_cluster_size` (default: 15), recursively split using higher threshold
5. For clusters smaller than `min_cluster_size` (default: 3), discard (too few memories to synthesize)
6. Compute quality_score for each cluster (average pairwise score of members)

### §2.4 Emotional Modulation

**Note**: Emotional modulation extends beyond stated requirements (requirements-1-engine.md §6.A) but is architecturally consistent with the memory system's emotional valence tracking.

Memories with emotional context receive special treatment throughout the synthesis pipeline:

**Signal Scoring Boost**
- Memories with `emotional_valence != None` and `|emotional_valence| > 0.0` receive a weight boost in pairwise signal scoring
- Boost factor: `1.0 + emotional_boost_weight × |emotional_valence|` (default `emotional_boost_weight`: 0.2)
- Applied multiplicatively to the composite score after weighted sum calculation
- Rationale: emotionally salient memories are more likely to form meaningful connections

**Cluster Prioritization**
- Each cluster computes an average emotional salience: mean of `|emotional_valence|` across members (0.0 for members with no emotional data)
- When clusters are sorted for gate processing and LLM budget allocation, high emotional-salience clusters are sorted before others (descending emotional salience, then by quality_score)
- This ensures emotionally significant knowledge is synthesized first under budget constraints

**LLM Prompt Context**
- When generating insights for clusters with emotional content, the prompt includes emotional context per memory:
  `[{id}: {content} (type: {memory_type}, importance: {importance}, emotion: {emotional_valence})]`
- The system prompt gains an additional instruction: "Consider the emotional significance of memories when forming insights. Emotional patterns (e.g., recurring positive/negative associations) are valid synthesis targets."

```rust
pub struct EmotionalModulationConfig {
    pub emotional_boost_weight: f64,    // multiplier for emotional valence boost, default: 0.2
    pub prioritize_emotional: bool,     // sort emotional clusters first, default: true
    pub include_emotion_in_prompt: bool, // add emotional context to LLM prompt, default: true
}
```

### §2.5 Candidate Pool Selection

Not all memories are candidates. Pre-filter:
- Must have been accessed at least once (access_count > 0)
- Must not already be a synthesis output (metadata tag `is_synthesis: true`)
- Must have importance ≥ min_importance (default: 0.3)
- Must not have been synthesized in the last N consolidation cycles (cooldown)

```rust
pub struct ClusterDiscoveryConfig {
    pub weights: ClusterWeights,
    pub cluster_threshold: f64,        // min pairwise score to link, default: 0.3
    pub min_cluster_size: usize,       // default: 3
    pub max_cluster_size: usize,       // default: 15
    pub min_importance: f64,           // default: 0.3
    pub temporal_decay_lambda: f64,    // for temporal proximity calc, default: 0.00413 (7-day half-life)
    pub temporal_half_life_hours: f64,  // half-life for temporal decay, default: 168.0 (7 days)
    pub cooldown_cycles: u32,          // skip recently-synthesized sources, default: 3
    pub temporal_spread_minimum: Duration, // min time spread across cluster members, default: 1 hour
}
```

---

## §3 Gate Check (GOAL-3)

### §3.1 Gate Decision Enum

**Note**: DEFER is a design extension to requirements' three-way classification (SYNTHESIZE/AUTO-UPDATE/SKIP). Recommended for addition to GOAL-3.

```rust
pub enum GateDecision {
    /// Cluster is rich enough to warrant LLM synthesis
    Synthesize { reason: String },
    /// Cluster can be merged without LLM (e.g., near-duplicate memories)
    AutoUpdate { action: AutoUpdateAction },
    /// Cluster exists but needs more memories before synthesis is worthwhile
    Defer { reason: String },
    /// Cluster is not worth processing
    Skip { reason: String },
}

pub enum AutoUpdateAction {
    /// Merge near-duplicate memories (keep highest importance)
    MergeDuplicates { keep: String, demote: Vec<String> },
    /// Strengthen existing hebbian links (no new insight needed)
    StrengthenLinks { pairs: Vec<(String, String)> },
}
```

### §3.2 Decision Logic (Zero-LLM)

The gate runs a decision tree on each cluster:

```
1. IF cluster has < min_cluster_size members → SKIP("too small")
2. IF all members have embedding similarity > 0.92 → AUTO-UPDATE(MergeDuplicates)
   (near-duplicates, no insight needed)
3. IF cluster quality_score < gate_quality_threshold → SKIP("low quality")
4. IF cluster members count == min_cluster_size AND quality_score < defer_quality_threshold → DEFER("cluster exists but needs more memories before synthesis is worthwhile")
   (cluster has minimum members but hasn't accumulated enough signal — revisit next cycle)
5. IF ≥80% of cluster members are already covered by an existing synthesis → SKIP("already-covered: ≥80% of source memories already have provenance records linking to an existing insight")
6. IF cluster has not grown since last synthesis attempt (same member set as previously deferred/skipped cluster) → SKIP("cluster-growth: cluster unchanged since last synthesis attempt, no new signal")
7. IF cluster members span < 2 distinct MemoryTypes → SKIP("homogeneous, unlikely to produce cross-domain insight")
   Exception: if all are Factual but entity_overlap is high, allow synthesis
   Exception: if all are Episodic and share a common topic (entity_overlap is high), allow synthesis
8. IF estimated LLM cost > cost_threshold AND cluster quality_score < premium_threshold → SKIP("not worth LLM cost")
9. ELSE → SYNTHESIZE
```

The "already-covered" check queries `synthesis_provenance` to see if ≥80% of memories in the cluster already appear as a `source_id` for an existing insight. If so, re-synthesizing would produce redundant knowledge.

The "cluster-growth" check compares the current cluster's deterministic ID (hash of sorted member IDs) against a persisted set of previously-attempted cluster IDs. If the ID matches a prior attempt and no new members have been added, the cluster is skipped.

Cost estimation:
- Approximate token count = sum of member content lengths / 4
- Cost = token_count × price_per_token (configurable)
- This is a rough guard, not precise billing

```rust
pub struct GateConfig {
    pub gate_quality_threshold: f64,    // min cluster quality to consider, default: 0.4
    pub defer_quality_threshold: f64,   // quality below which min-size clusters are deferred, default: 0.6
    pub duplicate_similarity: f64,      // threshold for auto-merge, default: 0.92
    pub min_type_diversity: usize,      // min distinct MemoryTypes, default: 2
    pub cost_threshold: f64,            // max estimated cost in USD, default: 0.05
    pub premium_threshold: f64,         // quality override for cost gate, default: 0.8
}
```

### §3.3 Gate Telemetry

Every gate decision is logged for observability:

```rust
pub struct GateResult {
    pub cluster_id: String,
    pub decision: GateDecision,
    pub scores: GateScores,
    pub timestamp: DateTime<Utc>,
}

pub struct GateScores {
    pub quality: f64,
    pub type_diversity: usize,
    pub estimated_cost: f64,
    pub member_count: usize,
}
```

---

## §4 Insight Generation (GOAL-2)

### §4.1 LLM Synthesis Pipeline

For clusters that pass the gate with SYNTHESIZE:

```rust
pub struct SynthesisRequest {
    pub cluster: MemoryCluster,
    pub members: Vec<MemoryRecord>,     // full records for prompt
    pub config: SynthesisConfig,
}

pub struct SynthesisConfig {
    pub model: String,                  // LLM model identifier
    pub max_tokens: usize,             // max output tokens, default: 512
    pub temperature: f64,              // default: 0.3 (low for factual synthesis)
    pub prompt_template: PromptTemplate,
    pub max_memories_per_llm_call: usize, // max source memories sent per LLM call, default: 10
    pub resynthesis_age_threshold: Duration, // re-synthesize insights older than this, default: 30 days
}

pub enum PromptTemplate {
    /// Default: general cross-memory synthesis
    General,
    /// For factual memories: extract common patterns/rules
    FactualPattern,
    /// For episodic memories: narrative thread synthesis
    EpisodicThread,
    /// For causal memories: causal chain discovery
    CausalChain,
}
```

### §4.2 Prompt Engineering

The prompt follows a structured format:

```
System: You are a knowledge synthesizer. Given a cluster of related memories,
produce a single higher-order insight that captures what these memories 
collectively reveal — a pattern, rule, or connection not explicit in any 
individual memory.

Rules:
- The insight must be a NEW observation, not a summary
- It must be falsifiable (can be proven wrong by future evidence)
- It must reference specific source memories by ID
- Keep it concise: 1-3 sentences for the insight, then supporting evidence

Format requirements:
- Respond ONLY with valid JSON — no markdown fences, no preamble, no trailing text
- All string values must be properly escaped
- "confidence" must be a decimal number between 0.0 and 1.0 (inclusive)
- "insight_type" must be exactly one of: "pattern", "rule", "connection", "contradiction"
- "source_references" must contain only IDs from the provided memories list
- "insight" must be between 50 and 500 characters

Memories:
[{id}: {content} (type: {memory_type}, importance: {importance})]
...

Required JSON schema:
{
  "insight": "string (50-500 chars, the synthesized insight text)",
  "confidence": "number (0.0-1.0)",
  "insight_type": "string (pattern|rule|connection|contradiction)",
  "source_references": ["string (memory IDs from input)"],
  "evidence": "string (brief explanation of how sources support this insight)"
}
```

Template selection based on dominant MemoryType in cluster:
- Majority Factual → FactualPattern
- Majority Episodic → EpisodicThread  
- Any Causal present → CausalChain
- Mixed/other → General

### §4.3 Output Validation

LLM output is validated before storage:

```rust
pub struct SynthesisOutput {
    pub insight_text: String,
    pub confidence: f64,
    pub insight_type: InsightType,
    pub source_ids: Vec<String>,
    pub evidence: String,
}

pub enum InsightType {
    Pattern,        // recurring theme across memories
    Rule,           // if-then relationship discovered
    Connection,     // link between seemingly unrelated memories
    Contradiction,  // conflicting information identified
}
```

Validation checks:
1. `source_ids` must all exist in the cluster members (no hallucinated references)
2. `confidence` must be in 0.0–1.0
3. `insight_text` must not be empty and must differ from any source content (not a copy)
4. `insight_type` must be a valid enum variant
5. `insight_text` length must be between 50 and 500 characters (too short = trivial, too long = not a synthesis)
6. Generated insight embedding must have >0.4 cosine similarity to the cluster centroid embedding (ensures the insight is semantically related to the source cluster, not a hallucinated tangent)
7. If any check fails → log error, do NOT store, do NOT demote sources (GUARD-1: No Data Loss)

**Design Note — Actionability (AC-2.1c)**: The requirement that insights be "actionable — an agent reading only the insight can make better decisions than reading any single source" is enforced via prompt engineering. The LLM prompt explicitly instructs: "The insight must be a NEW observation, not a summary" and "generate insights that enable better decision-making than reading any single source memory." This is a subjective criterion that cannot be validated programmatically, so it is addressed through careful prompt design rather than post-generation validation.

### §4.4 Insight Storage

A validated insight becomes a new MemoryRecord:

```rust
// Constructed from SynthesisOutput
MemoryRecord {
    id: generate_uuid(),
    content: output.insight_text,
    memory_type: MemoryType::Factual,  // insights are factual knowledge
    // metadata: { "is_synthesis": true, ... }  — tagged to distinguish from organic factual memories
    importance: compute_insight_importance(&output, &cluster),
    access_count: 0,
    created_at: Utc::now(),
    // ... other fields
}
```

Importance calculation:
- Base = average importance of source memories
- Boost by confidence: `base × (0.5 + 0.5 × confidence)`
- Boost by cluster quality: `× (0.8 + 0.2 × quality_score)`
- Cap at 1.0

### §4.5 Deterministic Scheduling (GUARD-5: Insight Identity)

Synthesis runs ONLY as part of `consolidate()`:
- No background threads, no timers, no cron
- Triggered by explicit `consolidate(days)` call
- Within consolidation: cluster discovery → gate → synthesis → provenance, in order
- If interrupted: no partial state (each insight is atomic: either fully stored with provenance, or not at all)

---

## §5 Provenance Tracing (GOAL-4)

### §5.1 Provenance Schema

New table for tracking insight-to-source relationships:

```sql
CREATE TABLE IF NOT EXISTS synthesis_provenance (
    id TEXT PRIMARY KEY,
    insight_id TEXT NOT NULL,           -- the synthesized memory ID
    source_id TEXT NOT NULL,            -- a source memory ID
    cluster_id TEXT NOT NULL,           -- which cluster produced this
    synthesis_timestamp TEXT NOT NULL,   -- when synthesis occurred
    gate_decision TEXT NOT NULL,         -- "SYNTHESIZE" (for audit)
    gate_scores TEXT,                   -- JSON: quality, cost, diversity
    confidence REAL NOT NULL,           -- LLM confidence in this insight
    source_original_importance REAL,    -- pre-demotion importance for reversibility
    FOREIGN KEY (insight_id) REFERENCES memories(id),
    FOREIGN KEY (source_id) REFERENCES memories(id)
);

CREATE INDEX idx_provenance_insight ON synthesis_provenance(insight_id);
CREATE INDEX idx_provenance_source ON synthesis_provenance(source_id);
```

**Multi-level provenance**: Since insights are stored as MemoryRecords, they can appear as source_id in subsequent synthesis provenance entries, enabling chains like: memories → insight A → insight B. The schema inherently supports this without modification. See NON-GOAL-2 in requirements for future multi-level synthesis scope.

### §5.2 Bidirectional Queries

```rust
impl Storage {
    /// Get all source memories that produced this insight
    pub fn get_insight_sources(&self, insight_id: &str) -> Result<Vec<ProvenanceRecord>> { ... }
    
    /// Get all insights derived from this source memory
    pub fn get_memory_insights(&self, source_id: &str) -> Result<Vec<ProvenanceRecord>> { ... }
    
    /// Get full provenance chain (recursive: insight → sources → their insights → ...)
    pub fn get_provenance_chain(&self, memory_id: &str, max_depth: usize) -> Result<ProvenanceChain> { ... }
}

pub struct ProvenanceRecord {
    pub insight_id: String,
    pub source_id: String,
    pub cluster_id: String,
    pub synthesis_timestamp: DateTime<Utc>,
    pub confidence: f64,
}

pub struct ProvenanceChain {
    pub root_id: String,
    pub layers: Vec<Vec<ProvenanceRecord>>,  // each layer = one synthesis depth
}
```

### §5.3 Reversibility (GUARD-1)

Undo synthesis = delete insight + restore source importance:

```rust
pub struct UndoSynthesis {
    pub insight_id: String,
    pub restored_sources: Vec<RestoredSource>,
}

pub struct RestoredSource {
    pub memory_id: String,
    pub original_importance: f64,   // stored in provenance at synthesis time
    pub restored: bool,
}
```

Process:
1. Look up all provenance records for insight_id
2. For each source: restore importance to pre-demotion value (stored in `source_original_importance`)
3. Delete the insight MemoryRecord
4. Delete all provenance records for this insight
5. This is a transaction — all or nothing (GUARD-1: No Data Loss)

---

## §6 Incremental Operations

### §6.1 New Memory Arrival

When new memories arrive after clusters have already been discovered (i.e., between consolidation cycles), the next synthesis run handles them incrementally:

**Scoring Against Existing Clusters**
1. For each new memory, compute pairwise composite scores against all members of each existing cluster
2. Derive a cluster-fit score = average pairwise score between the new memory and the cluster's members
3. If cluster-fit score ≥ `cluster_threshold` (same threshold as §2.3), the memory joins the cluster

**Cluster Membership Update**
- When a memory joins an existing cluster:
  - The cluster ID is recomputed (hash of sorted member IDs changes)
  - The cluster's `quality_score` is recalculated with the new member included
  - The cluster is re-evaluated by the gate (§3) — a previously DEFERRED cluster may now qualify for SYNTHESIZE
  - If the cluster previously produced an insight, the "cluster-growth" check (§3.2, rule 6) detects the changed member set and allows re-synthesis

**No Matching Cluster**
- If a new memory does not meet the threshold for any existing cluster, it is added to the general candidate pool
- It may seed a new cluster in the next full cluster discovery pass
- It remains eligible for future clustering as more related memories accumulate

**Synthesis Invalidation**
- When an existing cluster changes significantly (new members added, or source memories deleted/modified), existing syntheses derived from that cluster may become stale
- Significance threshold: if ≥50% of cluster members change, or if quality_score changes by >0.2, the existing insight is flagged for re-evaluation
- Re-evaluation does NOT auto-delete the old insight — it marks the provenance record with `stale: true` and allows the next synthesis run to produce an updated insight
- The old insight remains queryable until replaced (no silent data loss)

```rust
pub struct IncrementalConfig {
    pub staleness_member_change_pct: f64,   // % member change to flag stale, default: 0.5
    pub staleness_quality_delta: f64,       // quality_score change to flag stale, default: 0.2
}
```

---

## §7 Public API

### §7.1 Engine Trait

```rust
/// The synthesis engine — stateless computation, takes storage reference
pub trait SynthesisEngine {
    /// Run full synthesis pipeline on current memory state
    fn synthesize(&self, storage: &mut Storage, config: &SynthesisConfig) -> Result<SynthesisReport>;
    
    /// Cluster discovery only (for inspection/debugging)
    fn discover_clusters(&self, storage: &Storage, config: &ClusterDiscoveryConfig) -> Result<Vec<MemoryCluster>>;
    
    /// Gate check only (for inspection/debugging)
    fn check_gate(&self, cluster: &MemoryCluster, members: &[MemoryRecord], config: &GateConfig) -> GateResult;
    
    /// Undo a previous synthesis
    fn undo_synthesis(&self, storage: &mut Storage, insight_id: &str) -> Result<UndoSynthesis>;
    
    /// Query provenance
    fn get_provenance(&self, storage: &Storage, memory_id: &str, max_depth: usize) -> Result<ProvenanceChain>;
}
```

### §7.2 Synthesis Report

```rust
pub struct SynthesisReport {
    pub clusters_found: usize,
    pub clusters_synthesized: usize,
    pub clusters_auto_updated: usize,
    pub clusters_deferred: usize,
    pub clusters_skipped: usize,
    pub insights_created: Vec<String>,      // new insight memory IDs
    pub sources_demoted: Vec<String>,       // demoted source memory IDs
    pub errors: Vec<SynthesisError>,        // non-fatal errors (GUARD-1: No Data Loss — failures must not corrupt state)
    pub duration: std::time::Duration,
    pub gate_results: Vec<GateResult>,      // full telemetry
}

/// Non-fatal errors encountered during synthesis
pub enum SynthesisError {
    /// LLM call timed out for a cluster
    LlmTimeout { cluster_id: String },
    /// LLM returned unparseable response
    LlmInvalidResponse { cluster_id: String, raw_response: String },
    /// LLM referenced memory IDs not in the cluster
    HallucinatedReferences { cluster_id: String, invalid_ids: Vec<String> },
    /// Generated insight failed validation checks (§4.3)
    ValidationFailed { cluster_id: String, reason: String },
    /// Database operation failed
    StorageError { cluster_id: String, message: String },
    /// Embedding computation failed (e.g., model unavailable)
    EmbeddingError { memory_id: String, message: String },
    /// LLM budget exhausted mid-run
    BudgetExhausted { remaining_clusters: usize },
    /// Cluster was stale or changed between discovery and synthesis
    ClusterStale { cluster_id: String },
}
```

---

## §8 Configuration

All config integrates into existing `MemoryConfig`:

```rust
/// Added to MemoryConfig
pub struct SynthesisSettings {
    pub enabled: bool,                          // default: false (opt-in)
    pub cluster_discovery: ClusterDiscoveryConfig,
    pub gate: GateConfig,
    pub synthesis: SynthesisConfig,
    pub emotional: EmotionalModulationConfig,
    pub incremental: IncrementalConfig,
    pub demotion_factor: f64,                   // how much to reduce source importance, default: 0.5
    pub max_insights_per_consolidation: usize,  // rate limit, default: 5
    pub max_llm_calls_per_run: u32,             // hard cap on LLM calls per synthesis run (GUARD-2), default: 5
}
```

### §8.1 Configuration Defaults

| Parameter | Section | Default | Range/Type |
|-----------|---------|---------|------------|
| `enabled` | SynthesisSettings | `false` | bool |
| `demotion_factor` | SynthesisSettings | `0.5` | 0.0–1.0 |
| `max_insights_per_consolidation` | SynthesisSettings | `5` | usize |
| `max_llm_calls_per_run` | SynthesisSettings | `5` | u32 |
| `weights.hebbian` | ClusterDiscoveryConfig | `0.4` | 0.0–1.0 |
| `weights.entity` | ClusterDiscoveryConfig | `0.3` | 0.0–1.0 |
| `weights.embedding` | ClusterDiscoveryConfig | `0.2` | 0.0–1.0 |
| `weights.temporal` | ClusterDiscoveryConfig | `0.1` | 0.0–1.0 |
| `cluster_threshold` | ClusterDiscoveryConfig | `0.3` | 0.0–1.0 |
| `min_cluster_size` | ClusterDiscoveryConfig | `3` | usize |
| `max_cluster_size` | ClusterDiscoveryConfig | `15` | usize |
| `min_importance` | ClusterDiscoveryConfig | `0.3` | 0.0–1.0 |
| `temporal_decay_lambda` | ClusterDiscoveryConfig | `0.00413` | f64 |
| `temporal_half_life_hours` | ClusterDiscoveryConfig | `168.0` | f64 (hours) |
| `cooldown_cycles` | ClusterDiscoveryConfig | `3` | u32 |
| `temporal_spread_minimum` | ClusterDiscoveryConfig | `1 hour` | Duration |
| `emotional_boost_weight` | EmotionalModulationConfig | `0.2` | f64 |
| `prioritize_emotional` | EmotionalModulationConfig | `true` | bool |
| `include_emotion_in_prompt` | EmotionalModulationConfig | `true` | bool |
| `staleness_member_change_pct` | IncrementalConfig | `0.5` | 0.0–1.0 |
| `staleness_quality_delta` | IncrementalConfig | `0.2` | 0.0–1.0 |
| `gate_quality_threshold` | GateConfig | `0.4` | 0.0–1.0 |
| `defer_quality_threshold` | GateConfig | `0.6` | 0.0–1.0 |
| `duplicate_similarity` | GateConfig | `0.92` | 0.0–1.0 |
| `min_type_diversity` | GateConfig | `2` | usize |
| `cost_threshold` | GateConfig | `0.05` | f64 (USD) |
| `premium_threshold` | GateConfig | `0.8` | 0.0–1.0 |
| `model` | SynthesisConfig | (provider-specific) | String |
| `max_tokens` | SynthesisConfig | `512` | usize |
| `temperature` | SynthesisConfig | `0.3` | 0.0–1.0 |
| `prompt_template` | SynthesisConfig | `General` | PromptTemplate |
| `max_memories_per_llm_call` | SynthesisConfig | `10` | u32 |
| `resynthesis_age_threshold` | SynthesisConfig | `30 days` | Duration |

### §8.2 LLM Budget Enforcement (GUARD-2)

The `max_llm_calls_per_run` field enforces a hard cap on the number of LLM calls made during a single synthesis run. When the number of SYNTHESIZE-eligible clusters exceeds this budget, clusters are prioritized using the following sort order (highest priority first):

1. **Emotional valence** (highest): clusters whose members have the highest average emotional importance score
2. **Cluster size**: larger clusters represent more consolidated knowledge
3. **Temporal spread**: clusters spanning a wider time range indicate cross-temporal patterns

Clusters beyond the budget cap are deferred to the next consolidation cycle (they are not SKIPped — they remain eligible). The engine tracks `llm_calls_remaining` as a countdown during the pipeline and stops issuing SYNTHESIZE decisions once it reaches zero.

---

## §9 Error Handling (GUARD-1: No Data Loss)

Failure modes and responses:

| Failure | Response | Data Impact |
|---------|----------|-------------|
| LLM timeout | Skip cluster, log error | None |
| LLM returns invalid JSON | Skip cluster, log error | None |
| LLM returns hallucinated source IDs | Reject insight, log | None |
| Database write fails mid-synthesis | Rollback transaction | None |
| All LLM calls fail | Report in SynthesisReport.errors | None |

Key invariant: **No partial state.** Each insight creation is wrapped in a transaction:
1. BEGIN
2. INSERT insight memory
3. INSERT provenance records  
4. UPDATE source importances (demotion)
5. COMMIT

If any step fails → ROLLBACK. Sources remain at original importance. No insight stored.

---

## §10 Traceability Matrix

| Requirement | Design Section | Key Types |
|-------------|---------------|-----------|
| GOAL-1 (Cluster Discovery) | §2 | MemoryCluster, ClusterWeights, PairwiseSignals, EmotionalModulationConfig |
| GOAL-3 (Gate Check) | §3 | GateDecision, GateConfig, GateResult |
| GOAL-2 (Insight Generation) | §4 | SynthesisRequest, SynthesisOutput, InsightType |
| GOAL-4 (Provenance) | §5 | ProvenanceRecord, ProvenanceChain, UndoSynthesis |
| Incremental Operations | §6 | IncrementalConfig |
| GUARD-1 (No Data Loss) | §5.3, §9 | UndoSynthesis, demotion not deletion, reversibility, transaction rollback |
| GUARD-2 (LLM Cost Control) | §2, §3, §8.2 | No LLM in cluster/gate code paths, cost estimation gate, max_llm_calls_per_run budget |
| GUARD-3 (Backward Compatibility) | §7, §8 | Additive only, existing consolidate() unchanged, opt-in via SynthesisSettings (see master requirements.md) |
| GUARD-4 (No New External Dependencies) | §2, §3, §4 | Uses existing embeddings (§2), zero-LLM gate (§3), uses existing LLM provider (§4) |
| GUARD-5 (Insight Identity) | §4.4, §4.5 | Insights stored as MemoryRecords, metadata-tagged, participate in normal recall |
| GUARD-6 (Existing Signal Reuse) | §2, §3 | Gate uses only existing infrastructure: embeddings, entities, Hebbian, ACT-R signals |
