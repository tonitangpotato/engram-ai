# Design: ISS-016 — LLM Triple Extraction for Hebbian Link Quality

## 1. Overview

This design adds an LLM-powered triple extraction layer that enriches memories with `(Subject, Predicate, Object)` knowledge triples during consolidation. Extracted entities feed back into the Hebbian entity_overlap signal, replacing dictionary-only entity matching with a hybrid approach: fast Aho-Corasick for known terms + LLM for novel concepts and relationships.

**Key trade-off:** Zero hot-path impact (all LLM work happens in `consolidate()`) at the cost of delayed enrichment — newly stored memories don't have triples until the next consolidation cycle.

**Satisfies:** All 16 GOALs + 4 GUARDs from requirements.

---

## 2. Architecture

```
                           WRITE PATH (unchanged)
                           ─────────────────────
  text → store() → Aho-Corasick entities → memory_entities table
                          ↓
                   association discovery (existing)
                          ↓
                   entity_jaccard uses memory_entities (AC-only at write time)


                        CONSOLIDATE PATH (new)
                        ────────────────────────
  consolidate_namespace()
          ↓
    [existing] decay + rebalance + synthesis
          ↓
    [NEW] triple extraction phase
          ↓
    query: memories WHERE no triples AND retry_count < 3
          ↓
    batch (default 10) → release DB lock
          ↓
    for each memory: LLM call → parse JSON → Vec<Triple>
          ↓
    re-acquire DB lock → store triples + insert triple entities into memory_entities
          ↓
    next consolidation: association discovery sees enriched entity sets
```

---

## 3. Components

### 3.1 Triple Types (`src/triple.rs`)

*Satisfies: GOAL-1.2, GOAL-2.2*

New file defining the core triple types.

```rust
/// Standard predicate vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Predicate {
    IsA,
    PartOf,
    Uses,
    DependsOn,
    CausedBy,
    LeadsTo,
    Implements,
    Contradicts,
    RelatedTo,  // fallback for unrecognized predicates
}

impl Predicate {
    /// Parse from string, falling back to RelatedTo for unrecognized values.
    pub fn from_str_lossy(s: &str) -> Self {
        match s.to_lowercase().replace('-', "_").as_str() {
            "is_a" | "isa" => Self::IsA,
            "part_of" | "partof" => Self::PartOf,
            "uses" => Self::Uses,
            "depends_on" | "dependson" => Self::DependsOn,
            "caused_by" | "causedby" => Self::CausedBy,
            "leads_to" | "leadsto" => Self::LeadsTo,
            "implements" => Self::Implements,
            "contradicts" => Self::Contradicts,
            _ => Self::RelatedTo,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::IsA => "is_a",
            Self::PartOf => "part_of",
            Self::Uses => "uses",
            Self::DependsOn => "depends_on",
            Self::CausedBy => "caused_by",
            Self::LeadsTo => "leads_to",
            Self::Implements => "implements",
            Self::Contradicts => "contradicts",
            Self::RelatedTo => "related_to",
        }
    }
}

/// Source of triple extraction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TripleSource {
    Llm,
    Rule,
    Manual,
}

/// A knowledge triple extracted from a memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Triple {
    pub subject: String,
    pub predicate: Predicate,
    pub object: String,
    pub confidence: f64,  // 0.0-1.0, clamped on construction
    pub source: TripleSource,
}

impl Triple {
    pub fn new(subject: String, predicate: Predicate, object: String, confidence: f64) -> Self {
        Self {
            subject,
            predicate,
            object,
            confidence: confidence.clamp(0.0, 1.0),
            source: TripleSource::Llm,
        }
    }
}
```

### 3.2 Storage: Migration + CRUD (`src/storage.rs` additions)

*Satisfies: GOAL-1.1, GOAL-1.3, GOAL-1.4, GOAL-4.1, GUARD-1*

**Migration function** — follows the existing pattern (`migrate_v2`, `migrate_embeddings`, etc.). Called from `Storage::new()` after existing migrations.

```sql
-- migrate_triples()
CREATE TABLE IF NOT EXISTS triples (
    id INTEGER PRIMARY KEY,
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    predicate TEXT NOT NULL,
    object TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 1.0,
    source TEXT NOT NULL DEFAULT 'llm',
    created_at TEXT NOT NULL,
    UNIQUE(memory_id, subject, predicate, object)
);

CREATE INDEX IF NOT EXISTS idx_triples_memory ON triples(memory_id);
CREATE INDEX IF NOT EXISTS idx_triples_subject ON triples(subject);
CREATE INDEX IF NOT EXISTS idx_triples_object ON triples(object);
CREATE INDEX IF NOT EXISTS idx_triples_predicate ON triples(predicate);

-- Retry tracking: add column to memories table
ALTER TABLE memories ADD COLUMN triple_extraction_attempts INTEGER DEFAULT 0;
```

**New storage methods:**

```rust
impl Storage {
    /// Store triples for a memory. Duplicate (memory_id, s, p, o) are silently ignored.
    /// Also inserts triple subjects/objects as entities into memory_entities
    /// with source='triple' for transparent Hebbian integration.
    fn store_triples(&self, memory_id: &str, triples: &[Triple]) -> Result<usize>;

    /// Get triples for a memory.
    fn get_triples(&self, memory_id: &str) -> Result<Vec<Triple>>;

    /// Check if a memory has triples already extracted.
    fn has_triples(&self, memory_id: &str) -> Result<bool>;

    /// Get memory IDs that need triple extraction (no triples, retry_count < max).
    fn get_unenriched_memory_ids(&self, limit: usize, max_retries: u32) -> Result<Vec<String>>;

    /// Increment the extraction attempt counter for a memory.
    fn increment_extraction_attempts(&self, memory_id: &str) -> Result<()>;

    /// Migration function, called from Storage::new().
    fn migrate_triples(conn: &Connection) -> SqlResult<()>;
}
```

**Entity merge strategy** (GOAL-4.1): When `store_triples()` stores a triple, it also inserts the subject and object as entities in the existing `entities` + `memory_entities` tables. This way, `get_entities_for_memory()` transparently returns the union of AC-extracted and triple-extracted entities without changing any caller code.

```rust
// Inside store_triples(), for each triple:
fn insert_triple_entity(&self, memory_id: &str, entity_name: &str) -> Result<()> {
    let entity_id = format!("triple-{}", hash(entity_name));
    // Upsert into entities table
    self.conn.execute(
        "INSERT OR IGNORE INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at)
         VALUES (?1, ?2, 'concept', 'triple', '{}', ?3, ?3)",
        params![entity_id, entity_name.to_lowercase(), Utc::now().to_rfc3339()],
    )?;
    // Link to memory
    self.conn.execute(
        "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id, role) VALUES (?1, ?2, 'triple')",
        params![memory_id, entity_id],
    )?;
    Ok(())
}
```

### 3.3 LLM Triple Extractor (`src/triple_extractor.rs`)

*Satisfies: GOAL-2.1, GOAL-2.2, GOAL-2.3, GOAL-2.4*

New file. Uses the same `MemoryExtractor`-style pattern but with a dedicated trait for triple extraction.

```rust
/// Trait for triple extraction from memory text.
pub trait TripleExtractor: Send + Sync {
    /// Extract triples from memory content.
    /// Returns empty vec for no-op content. Errors are non-fatal.
    fn extract_triples(&self, content: &str) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>>;
}
```

**LLM prompt** (few-shot, constrained predicates):

```
You are a knowledge extraction system. Extract (subject, predicate, object) triples from the following text.

Rules:
- Extract concrete relationships, not vague associations
- Each triple should capture one specific relationship
- Normalize entity names to lowercase
- Use ONLY these predicates: is_a, part_of, uses, depends_on, caused_by, leads_to, implements, contradicts, related_to
- If unsure about the predicate, use "related_to"
- Rate confidence 0.0-1.0 for each triple
- If nothing meaningful can be extracted, return empty array
- Respond in JSON only, no markdown

Examples:

Input: "Engram uses ACT-R activation decay for memory strength calculation"
Output: [
  {"subject": "engram", "predicate": "uses", "object": "act-r activation decay", "confidence": 0.9},
  {"subject": "act-r activation decay", "predicate": "is_a", "object": "memory model", "confidence": 0.8}
]

Input: "The Infomap algorithm failed because the graph was disconnected"
Output: [
  {"subject": "infomap", "predicate": "is_a", "object": "clustering algorithm", "confidence": 0.9},
  {"subject": "disconnected graph", "predicate": "caused_by", "object": "graph structure", "confidence": 0.7},
  {"subject": "disconnected graph", "predicate": "leads_to", "object": "infomap failure", "confidence": 0.85}
]

Input: "ok sounds good"
Output: []

Text:
"{content}"
```

**Parsing:** Parse JSON response → for each object, map predicate string through `Predicate::from_str_lossy()` (unknown → `RelatedTo`), clamp confidence to [0.0, 1.0]. On parse failure: log warning, return empty vec (GOAL-2.3).

**Implementations:**

```rust
/// Anthropic-based triple extractor.
pub struct AnthropicTripleExtractor {
    api_key: String,
    model: String,  // default: claude-haiku (cheapest)
}

/// Ollama-based triple extractor (local, free).
pub struct OllamaTripleExtractor {
    model: String,  // default: llama3.2:3b
    url: String,
}
```

Both implement `TripleExtractor`. Follow the same HTTP client pattern as existing `AnthropicExtractor` / `OllamaExtractor` in `extractor.rs`.

### 3.4 Consolidation Integration (`src/models/consolidation.rs` + `src/memory.rs`)

*Satisfies: GOAL-3.1, GOAL-3.2, GOAL-3.3, GOAL-6.1, GUARD-2, GUARD-3*

Triple extraction runs as a new phase in `Memory::consolidate_namespace()`, **after** the existing consolidation cycle and synthesis, **outside** any database transaction.

```rust
// In Memory::consolidate_namespace():

// ... existing consolidation + decay + synthesis ...

// [NEW] Triple extraction phase (cold path, no DB lock during LLM calls)
if self.config.triple.enabled {
    if let Some(ref extractor) = self.triple_extractor {
        self.run_triple_extraction(extractor.as_ref(), namespace)?;
    }
}
```

**Lock-release-lock pattern** (GOAL-6.1):

```rust
fn run_triple_extraction(
    &mut self,
    extractor: &dyn TripleExtractor,
    namespace: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let batch_size = self.config.triple.batch_size;
    let max_retries = self.config.triple.max_retries;

    // Step 1: Query un-enriched memories (quick DB read, no lock held)
    let memory_ids = self.storage.get_unenriched_memory_ids(batch_size, max_retries)?;

    if memory_ids.is_empty() {
        return Ok(());
    }

    // Step 2: Read memory content (batch read)
    let mut memory_texts: Vec<(String, String)> = Vec::new();
    for id in &memory_ids {
        if let Ok(Some(record)) = self.storage.get(id) {
            memory_texts.push((id.clone(), record.content.clone()));
        }
    }

    // Step 3: LLM extraction — NO DB lock held, can take seconds per call
    let mut results: Vec<(String, Result<Vec<Triple>, Box<dyn Error + Send + Sync>>)> = Vec::new();
    for (id, content) in &memory_texts {
        let result = extractor.extract_triples(content);
        results.push((id.clone(), result));
    }

    // Step 4: Store results (DB write, transaction for atomicity)
    self.storage.begin_transaction()?;
    let write_result = (|| -> Result<(), Box<dyn Error>> {
        for (id, result) in &results {
            match result {
                Ok(triples) if !triples.is_empty() => {
                    self.storage.store_triples(id, triples)?;
                    log::debug!("Extracted {} triples for memory {}", triples.len(), id);
                }
                Ok(_) => {
                    // Empty triples — mark as attempted so we don't retry
                    self.storage.increment_extraction_attempts(id)?;
                }
                Err(e) => {
                    log::warn!("Triple extraction failed for {}: {}", id, e);
                    self.storage.increment_extraction_attempts(id)?;
                }
            }
        }
        Ok(())
    })();

    match write_result {
        Ok(()) => self.storage.commit_transaction()?,
        Err(e) => {
            let _ = self.storage.rollback_transaction();
            log::warn!("Triple storage failed (non-fatal): {}", e);
        }
    }

    Ok(())
}
```

**Retry tracking** (GOAL-3.2): `get_unenriched_memory_ids()` queries:
```sql
SELECT id FROM memories
WHERE id NOT IN (SELECT DISTINCT memory_id FROM triples)
  AND triple_extraction_attempts < ?1
ORDER BY created_at DESC
LIMIT ?2
```

Memories that fail extraction get `triple_extraction_attempts` incremented. After `max_retries` (default: 3), they're permanently skipped.

### 3.5 Configuration (`src/config.rs`)

*Satisfies: GOAL-5.1, GOAL-5.2, GOAL-3.3*

```rust
/// Configuration for LLM triple extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TripleConfig {
    /// Enable/disable triple extraction during consolidation
    pub enabled: bool,
    /// Number of memories to process per consolidation cycle
    pub batch_size: usize,
    /// Maximum extraction retry attempts before permanent skip
    pub max_retries: u32,
    /// LLM model override for triple extraction (None = use default extractor model)
    pub model: Option<String>,
}

impl Default for TripleConfig {
    fn default() -> Self {
        Self {
            enabled: false,  // opt-in, backward compatible
            batch_size: 10,
            max_retries: 3,
            model: None,
        }
    }
}
```

Add `pub triple: TripleConfig` to `MemoryConfig` with `#[serde(default)]`.

Add to `Memory` struct:
```rust
triple_extractor: Option<Box<dyn TripleExtractor>>,
```

With corresponding setter:
```rust
pub fn set_triple_extractor(&mut self, extractor: Box<dyn TripleExtractor>) {
    self.triple_extractor = Some(extractor);
}
```

### 3.6 Backward Compatibility (`src/storage.rs`)

*Satisfies: GOAL-4.2, GUARD-4*

- Migration is additive only (`CREATE TABLE IF NOT EXISTS`, `ALTER TABLE ADD COLUMN`) — no existing tables touched
- `ALTER TABLE ... ADD COLUMN` uses `IF NOT EXISTS` pattern (or catches the "duplicate column" error like existing migrations)
- `get_entities_for_memory()` is unchanged — it already queries the `memory_entities` join table, which now also contains triple-sourced entities
- If `TripleConfig::enabled` is false, `consolidate_namespace()` skips the entire extraction phase — zero overhead
- A pre-migration DB with no `triples` table: migration runs automatically on `Storage::new()` (existing pattern)

---

## 4. Data Flow

### 4.1 Write Path (unchanged)

```
Memory::add_to_namespace()
    → extract entities (Aho-Corasick) → store in memory_entities
    → generate embedding
    → association discovery (entity_jaccard uses memory_entities)
    → return memory ID
```

No triple extraction here. Hot path latency unchanged (GUARD-2).

### 4.2 Consolidation Path (new phase added)

```
Memory::consolidate_namespace()
    → run_consolidation_cycle()       [existing: decay, rebalance]
    → decay_hebbian_links()           [existing]
    → synthesis                        [existing]
    → run_triple_extraction()          [NEW]
        → get_unenriched_memory_ids(batch=10, max_retries=3)
        → for each: read content, release lock, LLM call
        → store triples + insert entities into memory_entities
```

### 4.3 Next Write After Enrichment

```
Memory::add_to_namespace() — new memory arrives
    → association discovery
    → CandidateSelector finds candidates
    → SignalComputer::entity_jaccard(entities_A, entities_B)
        → entities_B now includes triple-derived entities (via memory_entities)
        → entity overlap signal is stronger and more accurate
    → better link quality
```

---

## 5. Error Handling

| Scenario | Behavior | GOAL/GUARD |
|----------|----------|------------|
| LLM unavailable | Log warning, skip batch, retry next cycle | GUARD-3 |
| LLM returns invalid JSON | Parse fails → zero triples, log warning, increment attempts | GOAL-2.3 |
| LLM returns unknown predicate | Map to `RelatedTo` | GOAL-2.2 |
| Memory deleted during extraction | `ON DELETE CASCADE` removes any triples stored | GOAL-1.3 |
| Confidence outside [0.0, 1.0] | Clamped in `Triple::new()` | GOAL-1.2 |
| Same triple extracted twice | `UNIQUE` constraint → `INSERT OR IGNORE` | GOAL-1.3 |
| Extraction fails 3 times | `triple_extraction_attempts >= max_retries` → permanently skipped | GOAL-3.2 |
| DB lock contention | LLM calls happen outside any transaction | GOAL-6.1 |

---

## 6. Testing Strategy

| Test | Type | Validates |
|------|------|-----------|
| Predicate::from_str_lossy round-trips | Unit | GOAL-2.2 |
| Unknown predicate → RelatedTo | Unit | GOAL-2.2 |
| Triple::new clamps confidence | Unit | GOAL-1.2 |
| store_triples + get_triples round-trip | Integration | GOAL-1.1 |
| Duplicate triple is idempotent | Integration | GOAL-1.3 |
| ON DELETE CASCADE removes triples | Integration | GOAL-1.3 |
| migrate_triples on fresh DB | Integration | GOAL-1.4 |
| migrate_triples on existing DB (idempotent) | Integration | GOAL-1.4 |
| get_unenriched_memory_ids skips enriched | Integration | GOAL-3.2 |
| get_unenriched_memory_ids respects max_retries | Integration | GOAL-3.2 |
| Triple entities appear in get_entities_for_memory | Integration | GOAL-4.1 |
| entity_jaccard uses triple-derived entities | Integration | GOAL-4.1 |
| Memories without triples still work in Hebbian | Integration | GOAL-4.2 |
| Mock extractor returns error → graceful handling | Integration | GOAL-2.3, GUARD-3 |
| Mock extractor returns garbage JSON → zero triples | Integration | GOAL-2.3 |
| consolidate() with triple.enabled=false skips extraction | Integration | GOAL-5.1 |
| consolidate() does not call extractor during store() | Integration | GUARD-2 |
| TripleConfig defaults are correct | Unit | GOAL-3.3, GOAL-5.1 |

---

## 7. File Changes Summary

| File | Change | New/Modified |
|------|--------|-------------|
| `src/triple.rs` | Triple, Predicate, TripleSource types | New |
| `src/triple_extractor.rs` | TripleExtractor trait, Anthropic/Ollama impls, prompt | New |
| `src/storage.rs` | migrate_triples(), store_triples(), get_triples(), has_triples(), get_unenriched_memory_ids(), increment_extraction_attempts(), insert_triple_entity() | Modified |
| `src/config.rs` | TripleConfig struct, add to MemoryConfig | Modified |
| `src/memory.rs` | triple_extractor field, set_triple_extractor(), run_triple_extraction() in consolidate_namespace() | Modified |
| `src/lib.rs` | pub mod triple, pub mod triple_extractor, re-exports | Modified |
| `tests/triple_integration.rs` | Integration tests for full pipeline | New |
