# Design: Entity Indexing (ISS-009)

> Populate the empty `entities` / `memory_entities` / `entity_relations` tables
> so recall can do concept-level jumps instead of pure vector search.

## §1 Goals & Non-Goals

### Goals
- GOAL-1: Extract entities from memory content on every `add_raw()` call
- GOAL-2: Upsert entities into `entities` table (dedup by normalized name + type)
- GOAL-3: Link memories to entities via `memory_entities` junction table
- GOAL-4: Build entity↔entity relations from co-occurrence in same memory
- GOAL-5: Add entity-aware recall as 4th channel in hybrid search
- GOAL-6: Provide `backfill_entities()` to process existing memories
- GOAL-7: Keep `add_raw()` fast — regex extraction only, no LLM calls in hot path

### Non-Goals
- LLM-based entity extraction on write (too slow for hot path, can add later)
- Entity disambiguation (treating "gid" and "gid-core" as same entity)
- Entity deletion/cleanup UI
- Replacing existing hybrid search — this is additive
- Chinese entity extraction (deferred to LLM extraction phase; known entity lists can cover specific Chinese terms)

## §2 Module Structure

### New file: `src/entities.rs` (~400 lines)

Contains:
- `EntityExtractor` — regex/heuristic entity extraction (Aho-Corasick for known entity lists)
- `entity_recall()` — entity-aware retrieval logic
- Types: `Entity`, `EntityRelation`, `ExtractedEntity`

Entity CRUD methods are added directly to `Storage` (no separate trait — engram has one backend).

### Modified files:
- `src/memory.rs` — call entity extraction in `add_raw()`, integrate entity recall in `recall_from_namespace()`
- `src/storage.rs` — add entity CRUD methods
- `src/lib.rs` — `pub mod entities;`

## §3 Entity Extraction Strategy

### §3.1 Pattern-Based Extraction (hot path)

Fast regex/heuristic extraction, runs on every `add_raw()`:

```rust
pub struct EntityExtractor {
    /// Regex patterns for structural entities (ISS-\d+, file paths, URLs)
    patterns: Vec<EntityPattern>,
    /// Aho-Corasick automaton for known entity lists (projects, people, tech)
    /// Single O(n) scan over content for all known entities simultaneously
    known_matcher: aho_corasick::AhoCorasick,
    /// Maps match index → (entity_type, normalized_name)
    known_index: Vec<(EntityType, String)>,
}

struct EntityPattern {
    regex: Regex,
    entity_type: EntityType,
    name_group: usize,  // capture group index for entity name
}
```

**Built-in patterns:**

| Entity Type | Pattern | Examples |
|---|---|---|
| `project` | Known project names (configurable list) | rustclaw, engramai, gid-core, gid-rs, agentctl |
| `project` | Cargo crate names: `\b[a-z][a-z0-9_-]*-rs\b`, `\b[a-z][a-z0-9_]*ai\b` | infomap-rs, engramai |
| `person` | `@\w+`, known names list | @potatosoupup, potato |
| `technology` | Known tech terms (configurable) | Rust, SQLite, Telegram, Claude, Anthropic |
| `concept` | `ISS-\d+`, `GOAL-\d+`, `GUARD-\d+` | ISS-009, GOAL-1 |
| `file` | Path patterns: `src/\S+\.rs`, `\S+\.(rs\|py\|ts\|md)` | src/memory.rs |
| `url` | `https?://\S+` | https://crates.io/crates/engramai |

**Configurable known entities** via `MemoryConfig`:

```rust
pub struct EntityConfig {
    /// Known project names for high-confidence extraction
    pub known_projects: Vec<String>,
    /// Known person names
    pub known_people: Vec<String>,
    /// Known technology terms
    pub known_technologies: Vec<String>,
    /// Enable entity extraction (default: true)
    pub enabled: bool,
    /// Entity channel weight in hybrid search (default: 0.15)
    pub recall_weight: f64,
}
```

### §3.1.1 EntityExtractor Construction

```rust
impl EntityExtractor {
    pub fn new(config: &EntityConfig) -> Self {
        // 1. Build known entity list for Aho-Corasick
        let mut known_patterns = Vec::new();
        let mut known_index = Vec::new();
        
        for name in &config.known_projects {
            known_patterns.push(name.to_lowercase());
            known_index.push((EntityType::Project, name.to_lowercase()));
        }
        for name in &config.known_people {
            known_patterns.push(name.to_lowercase());
            known_index.push((EntityType::Person, name.to_lowercase()));
        }
        for name in &config.known_technologies {
            known_patterns.push(name.to_lowercase());
            known_index.push((EntityType::Technology, name.to_lowercase()));
        }
        
        let known_matcher = aho_corasick::AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(&known_patterns)
            .expect("valid patterns");
        
        // 2. Build regex patterns for structural entities
        let patterns = vec![
            // ISS-NNN, GOAL-NNN, GUARD-NNN
            EntityPattern { regex: Regex::new(r"\b(ISS-\d+|GOAL-\d+|GUARD-\d+)\b").unwrap(), entity_type: EntityType::Concept, name_group: 1 },
            // File paths
            EntityPattern { regex: Regex::new(r"\b(src/\S+\.rs|\S+\.(rs|py|ts|md))\b").unwrap(), entity_type: EntityType::File, name_group: 1 },
            // URLs
            EntityPattern { regex: Regex::new(r"(https?://\S+)").unwrap(), entity_type: EntityType::Url, name_group: 1 },
            // @mentions
            EntityPattern { regex: Regex::new(r"@(\w+)").unwrap(), entity_type: EntityType::Person, name_group: 1 },
            // Crate-like names (fallback, lower priority than known list)
            EntityPattern { regex: Regex::new(r"\b([a-z][a-z0-9_]*-rs)\b").unwrap(), entity_type: EntityType::Project, name_group: 1 },
        ];
        
        Self { patterns, known_matcher, known_index }
    }
    
    pub fn extract(&self, content: &str) -> Vec<ExtractedEntity> {
        let mut entities = Vec::new();
        let mut seen = HashSet::new(); // (normalized, type) dedup within same content
        
        // Phase 1: Aho-Corasick scan for known entities — O(n)
        for mat in self.known_matcher.find_iter(&content.to_lowercase()) {
            let (entity_type, normalized) = &self.known_index[mat.pattern()];
            let key = (normalized.clone(), entity_type.as_str().to_string());
            if seen.insert(key) {
                entities.push(ExtractedEntity {
                    name: content[mat.start()..mat.end()].to_string(),
                    normalized: normalized.clone(),
                    entity_type: entity_type.clone(),
                });
            }
        }
        
        // Phase 2: Regex patterns for structural entities
        for pattern in &self.patterns {
            for cap in pattern.regex.captures_iter(content) {
                if let Some(m) = cap.get(pattern.name_group) {
                    let name = m.as_str().to_string();
                    let normalized = normalize_entity_name(&name, &pattern.entity_type);
                    let key = (normalized.clone(), pattern.entity_type.as_str().to_string());
                    if seen.insert(key) {
                        entities.push(ExtractedEntity {
                            name,
                            normalized,
                            entity_type: pattern.entity_type.clone(),
                        });
                    }
                }
            }
        }
        
        entities
    }
}
```

### §3.2 Name Normalization

Before upserting, normalize entity names:
- Lowercase
- Strip leading `@` for person names
- Strip trailing `/` for URLs
- For file paths: keep relative path, strip absolute prefixes

Dedup key: `(normalized_name, entity_type, namespace)`

### §3.3 Extraction Output

```rust
pub struct ExtractedEntity {
    pub name: String,           // raw extracted name
    pub normalized: String,     // normalized for dedup
    pub entity_type: EntityType,
}

pub enum EntityType {
    Project,
    Person,
    Technology,
    Concept,
    File,
    Url,
    Organization,
    Other(String),
}
```

## §4 Storage API Additions

Add to `Storage` (src/storage.rs):

```rust
impl Storage {
    /// Upsert an entity. Returns entity ID (existing or new).
    pub fn upsert_entity(&self, name: &str, entity_type: &str, namespace: &str, metadata: Option<&str>) -> Result<String>;
    
    /// Link a memory to an entity.
    pub fn link_memory_entity(&self, memory_id: &str, entity_id: &str, role: &str) -> Result<()>;
    
    /// Add or strengthen an entity relation.
    pub fn upsert_entity_relation(&self, source_id: &str, target_id: &str, relation: &str, namespace: &str) -> Result<()>;
    
    /// Find entities matching a query by exact normalized name match.
    /// Exact match only — fuzzy/prefix matching is delegated to embedding search.
    pub fn find_entities(&self, query: &str, namespace: Option<&str>, limit: usize) -> Result<Vec<EntityRecord>>;
    // SQL: WHERE name = ? (exact match, not LIKE)
    
    /// Get all memories linked to an entity.
    pub fn get_entity_memories(&self, entity_id: &str) -> Result<Vec<String>>; // returns memory IDs
    
    /// Get related entities (1-hop).
    pub fn get_related_entities(&self, entity_id: &str, limit: usize) -> Result<Vec<(String, String)>>; // (entity_id, relation)
    
    /// Get entity by ID.
    pub fn get_entity(&self, id: &str) -> Result<Option<EntityRecord>>;
    
    /// Count entities in namespace.
    pub fn count_entities(&self, namespace: Option<&str>) -> Result<usize>;
}

pub struct EntityRecord {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub namespace: String,
    pub metadata: Option<String>,
    pub created_at: f64,
    pub updated_at: f64,
}
```

### §4.1 Entity ID Generation

`entity_id = hex(sha256(normalized_name + "|" + entity_type + "|" + namespace))[..16]`

16 hex characters = 64 bits. Birthday problem: ~4 billion entities before 50% collision probability — far beyond our scale (~10k entities max).

Deterministic — same entity always gets same ID. Upsert is idempotent.

**DB-level safety net:** `CREATE UNIQUE INDEX IF NOT EXISTS idx_entities_unique ON entities(name, entity_type, namespace)` — catches any normalization bugs that would create duplicate entities.

## §5 Integration into add_raw()

After current `add_raw()` stores the memory and embedding:

```rust
// In add_raw(), after embedding storage:

// Entity extraction (fast, regex-only)
if self.entity_config.enabled {
    let entities = self.entity_extractor.extract(content);
    for entity in &entities {
        let entity_id = self.storage.upsert_entity(
            &entity.normalized,
            &entity.entity_type.as_str(),
            ns,
            None,
        )?;
        self.storage.link_memory_entity(&id, &entity_id, "mention")?;
    }
    
    // Co-occurrence relations: entity pairs in same memory (capped at 10 to avoid O(n²))
    let co_entities = &entities[..entities.len().min(10)];
    if co_entities.len() >= 2 {
        for i in 0..co_entities.len() {
            for j in (i+1)..co_entities.len() {
                let id_a = /* entity_id for entities[i] */;
                let id_b = /* entity_id for entities[j] */;
                self.storage.upsert_entity_relation(
                    &id_a, &id_b, "co_occurs", ns
                )?;
            }
        }
    }
}
```

**Performance**: regex extraction on typical memory content (~100 chars) is <1ms. SQLite upserts are ~1ms each. For a memory with 3 entities: ~5ms total overhead on `add_raw()`.

## §6 Entity-Aware Recall

### §6.1 Query Entity Extraction

Same `EntityExtractor` runs on the recall query to find entities:

```rust
let query_entities = self.entity_extractor.extract(query);
```

### §6.2 Entity Lookup & Memory Retrieval

```rust
// For each extracted entity in query:
//   1. Find matching entities in DB (exact name match)
//   2. Get memory IDs linked to those entities
//   3. Get related entities (1-hop) → get their memory IDs too
//   4. Score each memory by entity relevance

fn entity_recall(
    &self,
    query: &str,
    namespace: Option<&str>,
    limit: usize,
) -> Result<HashMap<String, f64>> {
    let query_entities = self.entity_extractor.extract(query);
    let mut memory_scores: HashMap<String, f64> = HashMap::new();
    
    for qe in &query_entities {
        // Direct entity match
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
                    *memory_scores.entry(mid).or_insert(0.0) += 0.5; // weaker
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
```

### §6.3 Hybrid Search Integration

In `recall_from_namespace()`, add entity scores as 4th channel:

**Default weights** (rebalanced):
- FTS: 10% (was 15%)
- Embedding: 50% (was 60%)
- ACT-R: 25% (unchanged)
- Entity: 15% (new)

**Runtime normalization:** weights are always divided by their sum, so user configs with any weight combination produce correct 0.0-1.0 scores. Existing users who don't update their config (FTS=0.15 + Emb=0.60 + ACT-R=0.25 + Entity=0.15 = 1.15) get auto-normalized without silent degradation.

```rust
// In recall_from_namespace(), after computing fts_score, emb_score, actr_score:
let entity_scores = self.entity_recall(query, Some(ns), limit * 3)?;

// Normalize weights to sum=1.0 (handles any user config)
let total_weight = fts_weight + emb_weight + actr_weight + entity_weight;
let (fw, ew, aw, entw) = if total_weight > 0.0 {
    (fts_weight / total_weight, emb_weight / total_weight, actr_weight / total_weight, entity_weight / total_weight)
} else {
    (0.25, 0.25, 0.25, 0.25)
};

// For each candidate:
let entity_score = entity_scores.get(&memory.id).copied().unwrap_or(0.0);
let combined = fw * fts_score 
    + ew * emb_score 
    + aw * actr_score
    + entw * entity_score;
```

Make weights configurable via `MemoryConfig` (already has `fts_weight`, `embedding_weight`, `actr_weight`):
```rust
pub struct MemoryConfig {
    // ... existing fields ...
    pub entity_weight: f64,  // default 0.15
}
```

## §7 Backfill

```rust
impl MemoryManager {
    /// Extract entities from all existing memories that don't have entity links.
    /// Returns (processed_count, entity_count, relation_count).
    pub fn backfill_entities(&mut self, batch_size: usize) -> Result<(usize, usize, usize)> {
        let unlinked = self.storage.get_memories_without_entities(batch_size)?;
        let mut entity_count = 0;
        let mut relation_count = 0;
        
        for (memory, ns) in &unlinked {
            let entities = self.entity_extractor.extract(&memory.content);
            let mut entity_ids = Vec::new();
            
            for entity in &entities {
                let eid = self.storage.upsert_entity(
                    &entity.normalized,
                    &entity.entity_type.as_str(),
                    ns,  // namespace from backfill query (see below)
                    None,
                )?;
                self.storage.link_memory_entity(&memory.id, &eid, "mention")?;
                entity_ids.push(eid);
                entity_count += 1;
            }
            
            // Co-occurrence relations (capped at 10 entities to avoid O(n²))
            let cap = entity_ids.len().min(10);
            for i in 0..cap {
                for j in (i+1)..cap {
                    self.storage.upsert_entity_relation(
                        &entity_ids[i], &entity_ids[j], "co_occurs", ns
                    )?;
                    relation_count += 1;
                }
            }
        }
        
        Ok((unlinked.len(), entity_count, relation_count))
    }
}
```

Add `Storage` helper:
```rust
/// Returns (MemoryRecord, namespace) pairs for memories without entity links.
pub fn get_memories_without_entities(&self, limit: usize) -> Result<Vec<(MemoryRecord, String)>> {
    // SELECT m.*, COALESCE(m.namespace, 'default') as ns FROM memories m 
    // LEFT JOIN memory_entities me ON m.id = me.memory_id 
    // WHERE me.entity_id IS NULL 
    // LIMIT ?
}
```

## §8 CLI Integration

Add to `src/main.rs` (CLI):

```
engram entities              # list entities (top 20 by mention count)
engram entities --type project  # filter by type
engram entities backfill     # run backfill on existing memories
engram entities stats        # count entities, relations, links
```

## §9 Test Plan

### Unit tests (~15 tests in entities.rs):
1. Pattern extraction: project names, person names, tech terms, concepts, files, URLs
2. Name normalization: case, @-stripping, path normalization
3. Entity ID determinism: same input → same ID
4. Empty content → no entities
5. Mixed language content (中英混合)
6. Overlapping patterns (e.g., "gid-rs" matches both project pattern and tech)

### Integration tests (~10 tests in memory.rs or tests/):
7. add_raw() creates entity records
8. add_raw() creates memory_entities links
9. add_raw() creates co-occurrence relations
10. Entity dedup: same entity from two memories → one entity, two links
11. entity_recall() returns memories for matching entities
12. entity_recall() returns memories via 1-hop related entities
13. Hybrid search with entity channel changes ranking
14. backfill_entities() processes unlinked memories
15. Configurable known entities list works
16. Entity extraction disabled via config → no entities stored

### Edge cases:
17. Memory with 0 entities → no error, no links
18. Memory with 20+ entities → only creates relations for first N (cap at 10 to avoid O(n²) explosion)
19. Namespace isolation: entities in ns-A not visible in ns-B recall

## §10 File Size Estimate

| File | New Lines | Modified Lines |
|---|---|---|
| `src/entities.rs` (new) | ~350 | — |
| `src/storage.rs` | — | +120 (entity CRUD) |
| `src/memory.rs` | — | +80 (integration) |
| `src/config.rs` | — | +20 (EntityConfig) |
| `src/lib.rs` | — | +2 |
| `src/main.rs` | — | +40 (CLI commands) |
| `tests/entity_tests.rs` (new) | ~250 | — |
| **Total** | ~600 new | +262 modified |

~860 lines total.

## §11 Migration

Tables already exist with correct schema. One index addition needed:

```sql
CREATE UNIQUE INDEX IF NOT EXISTS idx_entities_unique ON entities(name, entity_type, namespace);
```

This is safe to run on existing (empty) tables. Add to `Storage::init()` alongside existing index creation.

Also update `upsert_entity()` SQL to refresh `updated_at` on conflict:
```sql
INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
ON CONFLICT(id) DO UPDATE SET updated_at = ?6, metadata = COALESCE(?5, metadata)
```

For `entity_relations`, use co-occurrence count as confidence:
```sql
INSERT INTO entity_relations (source_id, target_id, relation_type, confidence, namespace)
VALUES (?1, ?2, ?3, 0.1, ?4)
ON CONFLICT(source_id, target_id, relation_type) DO UPDATE SET
  confidence = MIN(confidence + 0.1, 1.0),
  updated_at = ?5
```
Each repeated co-occurrence bumps confidence by 0.1, capped at 1.0.

## §12 Rollout

1. Implement `entities.rs` + storage methods + unit tests
2. Integrate into `add_raw()` + integration tests
3. Integrate into `recall_from_namespace()` + recall tests
4. Add CLI commands
5. Run `backfill_entities()` on production DB (~8000 memories)
6. Monitor recall quality — adjust entity_weight if needed
