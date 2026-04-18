# Design: Knowledge Synthesis — Part 2: Integration Layer

**Version**: 1.0  
**Status**: Draft  
**Date**: 2026-04-09  
**Requirements**: requirements-2-integration.md (GOAL-5 through GOAL-8)

---

## 1. Overview

This document designs the integration surface for knowledge synthesis: how synthesized insights merge back into the memory system (source demotion), how users access synthesis (CLI + API), and how parameters are configured. All integration follows one principle: **insights are memories, not special artifacts**.

### Dependencies on Part 1

This design assumes Part 1 (Synthesis Engine) provides:
- `SynthesisCluster` — a group of memory IDs with cluster metadata
- `GateDecision` — SYNTHESIZE / AUTO_UPDATE / SKIP per cluster
- `SynthesisResult` — the output of a synthesis cycle (insights generated, gate decisions)
- Provenance tracking (insight → source memory IDs)

---

## 2. Source Demotion (GOAL-5)

### 2.1 Demotion Algorithm

After an insight is generated and validated (Part 1), sources are demoted in a single atomic transaction:

```
fn demote_sources(conn, insight_id, source_ids, config):
    tx = conn.begin_transaction()
    try:
        // 1. Record pre-demotion state for reversibility
        for id in source_ids:
            mem = get_memory(tx, id)
            store_demotion_record(tx, insight_id, id, {
                original_importance: mem.importance,
                original_layer: mem.layer,
                original_working_strength: mem.working_strength,
                original_core_strength: mem.core_strength,
            })
        
        // 2. Demote sources
        for id in source_ids:
            mem = get_memory(tx, id)
            new_importance = mem.importance * config.demotion_factor  // default 0.5
            update_memory(tx, id, {
                importance: new_importance,
                layer: demote_layer(mem.layer),  // core→working, working→archive
                metadata: merge(mem.metadata, {"demoted_by": insight_id})
            })
        
        // 3. Set insight importance ≥ max of original source importances
        max_source_importance = max(original importances)
        if insight.importance < max_source_importance:
            update_memory(tx, insight_id, {importance: max_source_importance})
        
        // 4. Create Hebbian links: insight ↔ each source
        for id in source_ids:
            record_coactivation(tx, insight_id, id)
        
        tx.commit()
    catch:
        tx.rollback()
        return Err(...)
```

### 2.2 Layer Demotion Rules

```
fn demote_layer(current: MemoryLayer) -> MemoryLayer:
    match current:
        Core    → Working
        Working → Archive
        Archive → Archive  // already lowest, no change
```

Sources are demoted one layer at a time, not straight to archive. Multiple synthesis cycles may gradually push a memory from core → working → archive.

### 2.3 Demotion Record Storage

Demotion records are stored in a `synthesis_provenance` table for reversibility:

```sql
CREATE TABLE IF NOT EXISTS synthesis_provenance (
    insight_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    original_importance REAL NOT NULL,
    original_layer TEXT NOT NULL,
    original_working_strength REAL NOT NULL,
    original_core_strength REAL NOT NULL,
    demoted_at TEXT NOT NULL,
    PRIMARY KEY (insight_id, source_id)
);
```

This is the only new table — it exists purely for reversibility (SC-6). Insights themselves are regular `memories` rows.

### 2.4 Reversibility (SC-6)

```
fn reverse_synthesis(conn, insight_id):
    tx = conn.begin_transaction()
    records = query("SELECT * FROM synthesis_provenance WHERE insight_id = ?", insight_id)
    for r in records:
        update_memory(tx, r.source_id, {
            importance: r.original_importance,
            layer: r.original_layer,
            working_strength: r.original_working_strength,
            core_strength: r.original_core_strength,
        })
        // Remove "demoted_by" from metadata
        remove_metadata_key(tx, r.source_id, "demoted_by")
    
    // Archive the insight (don't delete — GUARD-1)
    update_memory(tx, insight_id, {layer: Archive, importance: 0.0})
    
    delete("synthesis_provenance WHERE insight_id = ?", insight_id)
    tx.commit()
```

### 2.5 Insight Participation in Memory Dynamics

Insights are stored via the existing `add()` path with specific metadata:

```json
{
    "memory_type": "factual",
    "metadata": {
        "synthesis": true,
        "synthesis_type": "cluster|pattern|anomaly|narrative|entity|temporal",
        "source_ids": ["abc123", "def456", "ghi789"],
        "gate_decision": "synthesize",
        "confidence": 0.85,
        "cycle_id": "cycle-20260409-001"
    }
}
```

After creation, insights participate in ALL existing dynamics:
- ACT-R activation (access_times accumulate via normal recall)
- Ebbinghaus forgetting (working_strength decays normally)
- Murre-Chessa transfer (working → core via consolidation)
- Hebbian co-activation (links to sources + any future co-recalls)
- No special-casing anywhere in the recall or consolidation paths

---

## 3. CLI Access (GOAL-6)

### 3.1 New Subcommands

Added under the existing `engram` CLI:

```
engram synthesize [OPTIONS]
    --namespace <NS>        Namespace to synthesize (default: "default")
    --dry-run               Preview clusters and gate decisions, no changes
    --max-insights <N>      Override max insights per cycle (default: from config)
    --verbose               Show detailed gate check reasoning
    --db <PATH>             Database path

engram insights [OPTIONS]
    --namespace <NS>        Filter by namespace
    --limit <N>             Max results (default: 20)
    --type <TYPE>           Filter by synthesis type (cluster/pattern/anomaly/narrative/entity/temporal)
    --db <PATH>             Database path

engram insight <ID>
    --sources               Show source memories with original importance
    --provenance            Show full provenance chain
    --db <PATH>             Database path

engram sleep [OPTIONS]
    --namespace <NS>        Namespace (default: "default")
    --days <D>              Consolidation window (default: 7.0)
    --db <PATH>             Database path
```

### 3.2 Output Formats

**`engram synthesize --dry-run`**:
```
Cluster Discovery: found 12 clusters (47 memories)
Gate Check:
  SYNTHESIZE:  3 clusters (ready for LLM)
  AUTO_UPDATE: 2 clusters (existing insight covers)
  SKIP:        5 clusters (near-duplicate)
  DEFER:       2 clusters (too recent)

Clusters to synthesize:
  [1] 5 memories — "Rust debugging patterns" (hebbian-primary, max-strength: 0.82)
  [2] 4 memories — "potato's working style" (embedding-primary, centroid-sim: 0.89)
  [3] 3 memories — "financial freedom strategy" (entity-primary, shared: project, revenue)

Dry run — no changes made. Run without --dry-run to execute.
```

**`engram synthesize`** (actual run):
```
Synthesis Cycle [cycle-20260409-001]
  Duration:     2.3s (ex-LLM) + 4.1s (LLM) = 6.4s total
  Clusters:     12 discovered, 3 synthesized, 2 auto-updated, 7 skipped
  LLM calls:    3 / 5 max
  Insights:     3 created
  Demotions:    12 sources demoted
  
  [1] insight-a1b2c3 — "Rust debugging principles" (confidence: 0.87, 5 sources)
  [2] insight-d4e5f6 — "potato prefers action-oriented approach" (confidence: 0.91, 4 sources)
  [3] insight-g7h8i9 — "Revenue strategy: SaaS + tools" (confidence: 0.78, 3 sources)
```

**`engram insights`**:
```
ID          Type      Sources  Confidence  Created      Content
insight-a1  cluster   5        0.87        2026-04-09   Rust debugging principles: ...
insight-d4  cluster   4        0.91        2026-04-09   potato prefers action-orient...
insight-g7  pattern   3        0.78        2026-04-08   Revenue strategy: SaaS + too...
```

**`engram insight <ID> --sources`**:
```
Insight: insight-a1b2c3
Type:    cluster
Created: 2026-04-09T14:30:00Z
Content: Rust debugging principles: (1) always read the full error...

Sources (5):
  ID        Original Imp.  Current Imp.  Layer     Content
  mem-001   0.60           0.30          archive   "When debugging Rust, first..."
  mem-002   0.45           0.22          archive   "cargo test -- --nocapture is..."
  mem-003   0.55           0.27          working   "The borrow checker error..."
  mem-004   0.40           0.20          archive   "Use RUST_BACKTRACE=1 for..."
  mem-005   0.50           0.25          archive   "dbg!() macro is faster than..."
```

### 3.3 `sleep` Command (Unified Cycle)

`engram sleep` runs the unified sleep cycle (Note D):

```
fn sleep_cycle(engine, namespace, days):
    // Phase 1: Synaptic consolidation (existing)
    engine.consolidate_namespace(namespace, days)?
    
    // Phase 2: Systems consolidation (new)
    if engine.has_extractor():  // LLM available
        result = engine.synthesize(namespace, SynthesisConfig::default())?
        return SleepResult { consolidation: ok, synthesis: Some(result) }
    else:
        return SleepResult { consolidation: ok, synthesis: None }
```

---

## 4. Programmatic API (GOAL-7)

### 4.1 Public API Surface

New methods on `MemoryEngine`:

```rust
impl MemoryEngine {
    /// Run a synthesis cycle. Returns structured results.
    pub fn synthesize(
        &mut self,
        namespace: &str,
        config: SynthesisConfig,
    ) -> Result<CycleReport, Box<dyn std::error::Error>>;

    /// Dry run: discover clusters and gate decisions without changes.
    pub fn synthesize_dry_run(
        &self,
        namespace: &str,
        config: SynthesisConfig,
    ) -> Result<DryRunReport, Box<dyn std::error::Error>>;

    /// Unified sleep cycle: consolidate() then synthesize().
    pub fn sleep_cycle(
        &mut self,
        namespace: &str,
        days: f64,
        synthesis_config: Option<SynthesisConfig>,
    ) -> Result<SleepReport, Box<dyn std::error::Error>>;

    /// List all insights, optionally filtered.
    pub fn list_insights(
        &self,
        namespace: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<MemoryRecord>, Box<dyn std::error::Error>>;

    /// Get source memories for an insight.
    pub fn insight_sources(
        &self,
        insight_id: &str,
    ) -> Result<Vec<ProvenanceEntry>, Box<dyn std::error::Error>>;

    /// Get insights derived from a memory.
    pub fn insights_for_memory(
        &self,
        memory_id: &str,
    ) -> Result<Vec<MemoryRecord>, Box<dyn std::error::Error>>;

    /// Reverse a synthesis: restore sources, archive insight.
    pub fn reverse_synthesis(
        &mut self,
        insight_id: &str,
    ) -> Result<ReversalReport, Box<dyn std::error::Error>>;
}
```

### 4.2 Return Types

```rust
/// Full cycle report for observability (GOAL-7 + Note G).
pub struct CycleReport {
    pub cycle_id: String,
    pub namespace: String,
    pub clusters_discovered: usize,
    pub gate_decisions: GateStats,
    pub llm_calls_made: usize,
    pub llm_calls_max: usize,
    pub insights_created: Vec<InsightSummary>,
    pub sources_demoted: usize,
    pub duration_discovery: Duration,
    pub duration_gate: Duration,
    pub duration_llm: Duration,
    pub duration_total: Duration,
}

pub struct GateStats {
    pub synthesize: usize,
    pub auto_update: usize,
    pub skip: usize,
    pub defer: usize,
}

pub struct InsightSummary {
    pub id: String,
    pub synthesis_type: String,
    pub confidence: f32,
    pub source_count: usize,
    pub content_preview: String,  // first 100 chars
}

pub struct DryRunReport {
    pub clusters: Vec<ClusterPreview>,
    pub gate_decisions: GateStats,
}

pub struct ClusterPreview {
    pub memory_ids: Vec<String>,
    pub signal: String,           // "hebbian" | "embedding" | "entity"
    pub strength: f32,            // max link strength or centroid similarity
    pub topic_hint: String,       // first 50 chars of centroid memory
    pub gate_decision: String,
}

pub struct SleepReport {
    pub consolidation_ok: bool,
    pub synthesis: Option<CycleReport>,
}

pub struct ProvenanceEntry {
    pub source: MemoryRecord,
    pub original_importance: f64,
    pub original_layer: MemoryLayer,
    pub demoted_at: DateTime<Utc>,
}

pub struct ReversalReport {
    pub insight_id: String,
    pub sources_restored: usize,
}
```

### 4.3 Recall Integration (Note H)

No changes to existing `recall()` or `recall_from_namespace()`. Insights naturally rank higher because:

1. **Higher importance** — set to max(source importances) at creation
2. **Better embeddings** — LLM-generated summaries are more focused than raw memories
3. **ACT-R activation** — fresh creation timestamp gives recency boost

Callers can distinguish insights from regular memories via metadata:

```rust
fn is_insight(record: &MemoryRecord) -> bool {
    record.metadata
        .as_ref()
        .and_then(|m| m.get("synthesis"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}
```

No separate query path needed. `recall()` returns insights mixed with regular memories, ranked by the same hybrid scoring.

---

## 5. Configuration (GOAL-8)

### 5.1 SynthesisConfig Struct

```rust
/// Configuration for knowledge synthesis.
/// Separate from MemoryConfig to keep concerns clean.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisConfig {
    // === Cluster Discovery ===
    /// Minimum cluster size (default: 3)
    pub min_cluster_size: usize,
    /// Maximum cluster size before splitting (default: 15)
    pub max_cluster_size: usize,
    /// Hebbian link strength threshold (default: 0.3)
    pub hebbian_threshold: f64,
    /// Embedding similarity threshold for clustering (default: 0.75)
    pub embedding_similarity_threshold: f64,
    /// Minimum shared entities for entity-based clustering (default: 2)
    pub entity_overlap_min: usize,

    // === Gate Check ===
    /// Intra-cluster similarity above this = near-duplicate, SKIP (default: 0.92)
    pub gate_duplicate_threshold: f64,
    /// Temporal spread minimum in hours (default: 1.0)
    pub gate_temporal_spread_hours: f64,
    /// Existing insight coverage threshold for SKIP (default: 0.80)
    pub gate_coverage_threshold: f64,
    /// Cluster growth threshold for RE-SYNTHESIZE (default: 0.50)
    pub gate_regrowth_threshold: f64,

    // === Synthesis ===
    /// Max LLM calls per cycle (default: 5)
    pub max_llm_calls: usize,
    /// Max memories sent in a single LLM call (default: 10)
    pub max_memories_per_call: usize,
    /// Max insights produced per cycle (default: 5)
    pub max_insights_per_cycle: usize,

    // === Demotion ===
    /// Importance multiplier for demoted sources (default: 0.5)
    pub demotion_factor: f64,

    // === Recency ===
    /// Number of recent consolidation cycles for recency boost (default: 3)
    pub recency_window_cycles: usize,
}
```

### 5.2 Defaults

```rust
impl Default for SynthesisConfig {
    fn default() -> Self {
        Self {
            min_cluster_size: 3,
            max_cluster_size: 15,
            hebbian_threshold: 0.3,
            embedding_similarity_threshold: 0.75,
            entity_overlap_min: 2,
            gate_duplicate_threshold: 0.92,
            gate_temporal_spread_hours: 1.0,
            gate_coverage_threshold: 0.80,
            gate_regrowth_threshold: 0.50,
            max_llm_calls: 5,
            max_memories_per_call: 10,
            max_insights_per_cycle: 5,
            demotion_factor: 0.5,
            recency_window_cycles: 3,
        }
    }
}
```

### 5.3 Integration with MemoryConfig

`SynthesisConfig` is **not** embedded in `MemoryConfig`. It's passed as a parameter to `synthesize()` and `sleep_cycle()`. Rationale:

- GUARD-3 (backward compat): existing MemoryConfig users see zero changes
- Synthesis is opt-in, not always-on
- Different calls may use different configs (e.g., aggressive vs conservative thresholds)

For CLI, the config can be loaded from a TOML/JSON file via `--config` flag, or defaults are used.

---

## 6. Module Structure

New files:

```
src/
  synthesis/
    mod.rs          — pub use, SynthesisConfig, SynthesisType
    demotion.rs     — demote_sources(), reverse_synthesis(), demotion records
    types.rs        — CycleReport, DryRunReport, GateStats, etc.
```

Integration points (modifications to existing files):

```
src/memory.rs       — add synthesize(), sleep_cycle(), list_insights(), 
                      insight_sources(), insights_for_memory(), reverse_synthesis()
                      (thin wrappers calling into synthesis/ modules)
src/main.rs (CLI)   — add synthesize, insights, insight, sleep subcommands
```

Part 1 (engine) will own:
```
src/
  synthesis/
    cluster.rs      — cluster discovery algorithm
    gate.rs         — gate check logic
    generate.rs     — LLM insight generation
    provenance.rs   — provenance queries
```

---

## 7. Error Handling

All synthesis operations return `Result<T, Box<dyn std::error::Error>>`, consistent with existing MemoryEngine API.

Specific error conditions:
- **No LLM configured**: `synthesize()` returns `Ok(CycleReport)` with zero insights and a warning field. Not an error — gate check and cluster discovery still run (Note F from Part 1).
- **Transaction failure**: rollback, return error. No partial state.
- **Invalid insight_id in reverse_synthesis**: return error "insight not found" or "not a synthesis insight".
- **Empty namespace**: return `Ok(CycleReport)` with zero clusters. Not an error.

---

## 8. Design Decisions & Trade-offs

### D1: Separate SynthesisConfig vs embedded in MemoryConfig
**Chose**: Separate.  
**Trade-off**: Users must pass config explicitly. But GUARD-3 is satisfied (zero changes to existing code), and different synthesis runs can use different configs.

### D2: One new table (synthesis_provenance) vs metadata-only
**Chose**: One new table for reversibility data.  
**Trade-off**: Adds a table (light schema change), but enables O(1) reversal lookups and clean atomic rollback. Storing original importance/layer in JSON metadata would be fragile and hard to query.

### D3: Insights as regular memories vs separate storage
**Chose**: Regular memories with metadata tags (GUARD-5).  
**Trade-off**: No separate query optimization for insights. But insights participate in all existing dynamics (ACT-R, Hebbian, consolidation) without any code changes to those systems.

### D4: sleep_cycle() as thin wrapper vs deep integration
**Chose**: Thin wrapper calling consolidate() then synthesize().  
**Trade-off**: Two separate transactions instead of one mega-transaction. But keeps consolidation and synthesis independently testable, and either can fail without blocking the other.
