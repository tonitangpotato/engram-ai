# Design: Memory Supersession

## §1 Overview

This feature adds a **supersession mechanism** to engramai that filters old/wrong memories from recall results. Unlike the existing `contradicted_by` penalty (which downranks), supersession **excludes** memories from the candidate set entirely.

### Requirements Coverage

| GOAL | Section |
|------|---------|
| GOAL-ss.1 (Supersede API) | §2.1 |
| GOAL-ss.2 (Recall Pre-Filter) | §2.2 (covers: recall, recall_from_namespace, hybrid_recall, recall_recent, recall_with_associations, recall_associated) |
| GOAL-ss.3 (Single Correction) | §2.3 |
| GOAL-ss.4 (Bulk Supersession) | §2.1 |
| GOAL-ss.5 (Bulk Correction) | §2.3 |
| GOAL-ss.6 (Auto-Detection) | §2.4 |
| GOAL-ss.7 (Chain Resolution) | §2.2 |
| GOAL-ss.8 (Undo) | §2.1 |
| SEC-ss.1 (Namespace Scope) | §2.1 |
| OBS-ss.1 (List Superseded) | §2.5 |
| GUARD-ss.1–4 | §3 |

### Key Trade-off

**SQL-level vs Rust-level filtering.** Two options:

1. Add `AND (superseded_by IS NULL OR superseded_by = '')` to every SQL query in `Storage`
2. Add a post-fetch filter in `Memory` that strips superseded records before scoring

Option 1 is more efficient (DB does the work, fewer rows transferred) but requires touching every SQL query (~12 queries across `search_fts`, `search_fts_ns`, `fetch_recent`, `all`, `all_in_namespace`, etc.). Option 2 is a single choke point but loads superseded rows only to discard them.

**Decision: Option 1 (SQL-level) for the primary recall paths, with a Rust-level safety net.** The SQL filter goes into `Storage` queries. `Memory::recall_from_namespace` and `Memory::recall_fts` already assemble candidates from `Storage` — adding the SQL clause there catches 90% of traffic. For the remaining paths (e.g., `storage.get()` used in candidate assembly), a Rust-level `.filter()` in `Memory` acts as defense-in-depth.

---

## §2 Components

### §2.1 Storage Layer: Schema + CRUD

**Schema migration.** On `Storage::new()`, after existing schema creation, run:

```sql
-- Idempotent: safe to run on existing DBs
ALTER TABLE memories ADD COLUMN superseded_by TEXT DEFAULT '';
```

Wrapped in a try-catch — if the column already exists (re-run), `ALTER TABLE` fails silently. This satisfies GUARD-ss.3.

**`row_to_record` update.** Read the new `superseded_by` column and populate `MemoryRecord.superseded_by`. Same pattern as existing `contradicted_by`:

```rust
let superseded_by_str: String = row.get("superseded_by")?;
// ...
superseded_by: if superseded_by_str.is_empty() { None } else { Some(superseded_by_str) },
```

**New methods on `Storage`:**

```rust
/// Mark old_id as superseded by new_id.
/// Validates: old_id exists, new_id exists, old_id != new_id.
/// If old_id is already superseded, updates the link (last-write-wins).
/// Sets superseded_by = new_id on the old memory row.
pub fn supersede(&self, old_id: &str, new_id: &str) -> Result<(), SupersessionError>

/// Supersede multiple old IDs with one new ID. Transactional.
/// If any old_id doesn't exist, rolls back and returns error with invalid IDs.
/// Empty old_ids = no-op success.
pub fn supersede_bulk(&self, old_ids: &[&str], new_id: &str) -> Result<usize, SupersessionError>

/// Clear superseded_by for a memory, restoring it to active recall.
pub fn unsupersede(&self, id: &str) -> Result<(), SupersessionError>

/// List all superseded memories, with their replacement ID.
/// Returns Vec<(superseded_record, replacement_id)>.
pub fn list_superseded(&self, namespace: Option<&str>) -> Result<Vec<(MemoryRecord, String)>, rusqlite::Error>
```

**`supersede()` implementation:**

```rust
pub fn supersede(&self, old_id: &str, new_id: &str) -> Result<(), SupersessionError> {
    if old_id == new_id {
        return Err(SupersessionError::SelfSupersession(old_id.to_string()));
    }
    // Validate old exists
    if self.get(old_id)?.is_none() {
        return Err(SupersessionError::NotFound(old_id.to_string()));
    }
    // Validate new exists
    if self.get(new_id)?.is_none() {
        return Err(SupersessionError::NotFound(new_id.to_string()));
    }
    // Namespace check (SEC-ss.1): both must be in the same namespace,
    // or the caller must have access to both. For the library layer,
    // we enforce same-namespace only. Cross-namespace requires
    // the caller to use the ACL-aware Memory layer.
    let old_ns = self.get_namespace(old_id)?;
    let new_ns = self.get_namespace(new_id)?;
    if old_ns != new_ns {
        return Err(SupersessionError::CrossNamespace {
            old_ns: old_ns.unwrap_or_default(),
            new_ns: new_ns.unwrap_or_default(),
        });
    }
    
    self.conn.execute(
        "UPDATE memories SET superseded_by = ? WHERE id = ?",
        params![new_id, old_id],
    )?;
    Ok(())
}
```

**Note on TOCTOU:** There's a theoretical race between `get()`/`get_namespace()` and the `UPDATE` — if a memory is deleted between validation and update, the UPDATE silently affects 0 rows. This is acceptable: superseding a just-deleted memory is a no-op with no harmful side effects. The alternative (single transactional query) adds complexity for a harmless edge case.

**`supersede_bulk()` implementation:** Uses `SAVEPOINT` for transactional semantics. Validates all IDs exist first, then updates in batch. Any missing ID → rollback + error listing invalids.

**Error type:**

```rust
#[derive(Debug, thiserror::Error)]
pub enum SupersessionError {
    #[error("Memory not found: {0}")]
    NotFound(String),
    
    #[error("Cannot supersede a memory with itself: {0}")]
    SelfSupersession(String),
    
    #[error("Cross-namespace supersession not allowed: {old_ns} → {new_ns}")]
    CrossNamespace { old_ns: String, new_ns: String },
    
    #[error("Bulk supersession failed — invalid IDs: {0:?}")]
    InvalidIds(Vec<String>),
    
    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),
}
```

### §2.2 Recall Pre-Filter

**SQL-level filter.** Add `AND (superseded_by IS NULL OR superseded_by = '')` to every `SELECT` query in `Storage` that feeds recall pipelines:

| Method | Current WHERE | Added clause |
|--------|---------------|--------------|
| `search_fts()` | `deleted_at IS NULL` | `AND (superseded_by IS NULL OR superseded_by = '')` |
| `search_fts_ns()` (both branches) | `deleted_at IS NULL` | same |
| `fetch_recent()` (default branch) | `namespace = ?` | `AND (superseded_by IS NULL OR superseded_by = '') AND deleted_at IS NULL` |
| `fetch_recent()` (wildcard `*` branch) | *(none — pre-existing bug)* | `WHERE (superseded_by IS NULL OR superseded_by = '') AND deleted_at IS NULL` ⚠️ also fixes missing `deleted_at` filter |
| `all()` | `deleted_at IS NULL` | same |
| `all_in_namespace()` | `namespace = ?` | same |
| `search_by_type()` | `memory_type = ?` | same |
| `search_by_type_ns()` | `memory_type = ? AND namespace = ?` | same |
| `hybrid_search` module queries | varies | `AND (superseded_by IS NULL OR superseded_by = '')` — check `hybrid_search.rs` for exact SQL |

**Note on `IS NULL OR = ''`:** The column default is `''` and code always writes strings (never SQL NULL), so `= ''` alone would suffice. The `IS NULL` check is pure defense — if a row is inserted by external tooling or a future code path that doesn't set the column, `IS NULL` catches it. Both checks together cost negligible overhead.

**Note:** `storage.get(id)` does NOT get the filter — it's used for direct lookups (e.g., validating IDs in `supersede()`). If a caller explicitly asks for a specific ID, they get it regardless of supersession status.

**Rust-level safety net in `Memory`.** Every `Memory`-level recall method that returns results to the caller gets a `.retain()` filter. This is defense-in-depth — if any SQL filter is missed or buggy, the Rust layer catches it:

| Memory method | Safety net location |
|---------------|-------------------|
| `recall_from_namespace()` | After candidate assembly, before scoring |
| `recall_recent()` | After `storage.fetch_recent()`, before return |
| `hybrid_recall()` | After `hybrid_search()` returns, before return |
| `recall_associated_ns()` | After `storage.search_by_type_ns()` call in the `cause_query=None` branch, before return |
| `recall_with_associations()` | After results assembled, before return |

```rust
// Applied in each method above:
// Defense-in-depth: filter out any superseded memories that slipped through
results.retain(|r| r.superseded_by.is_none());
// (or for RecallResult: results.retain(|r| r.record.superseded_by.is_none()))
```

**Chain resolution (GOAL-ss.7).** Chains are handled implicitly by the SQL filter: if A.superseded_by = B, A is excluded. If B.superseded_by = C, B is also excluded. Only C (superseded_by = '') passes. No recursive resolution needed in the normal recall path — each memory's `superseded_by` field is checked independently.

The only case where chain awareness matters is `list_superseded()` (OBS-ss.1), where we want to show the full chain for debugging. That method does iterative traversal:

```rust
/// Resolve the supersession chain head for a given memory.
/// Returns the final non-superseded memory ID, or None if cycle detected.
pub fn resolve_chain_head(&self, id: &str) -> Result<Option<String>, rusqlite::Error> {
    let mut current = id.to_string();
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current.clone()) {
            // Cycle detected
            log::warn!("Supersession cycle detected involving {}", current);
            return Ok(None);
        }
        match self.get(&current)? {
            Some(record) => match &record.superseded_by {
                Some(next) => current = next.clone(),
                None => return Ok(Some(current)),
            },
            None => return Ok(None), // broken chain
        }
    }
}
```

### §2.3 Correction Operations

**`Memory::correct()` — single correction (GOAL-ss.3):**

```rust
pub fn correct(
    &mut self,
    old_id: &str,
    new_content: &str,
    importance_override: Option<f64>,
    memory_type_override: Option<MemoryType>,
) -> Result<String, Box<dyn std::error::Error>>
```

Implementation:
1. Fetch old memory via `storage.get(old_id)` → error if not found
2. Get namespace via `storage.get_namespace(old_id)`
3. Determine memory_type: `memory_type_override.unwrap_or(old.memory_type)`
4. Determine importance: `importance_override.unwrap_or(old.importance.max(0.5))`
5. Call `self.add_to_namespace(new_content, memory_type, Some(importance), ...)` → get `new_id`
6. Call `self.storage.supersede(old_id, &new_id)` 
7. Return `new_id`

**`Memory::correct_bulk()` — bulk correction (GOAL-ss.5):**

```rust
pub fn correct_bulk(
    &mut self,
    query: &str,
    new_content: &str,
    namespace: Option<&str>,
    limit: usize,
) -> Result<BulkCorrectionResult, Box<dyn std::error::Error>>

pub struct BulkCorrectionResult {
    pub new_id: String,
    pub superseded_count: usize,
    pub superseded_ids: Vec<String>,
}
```

Implementation:
1. Search for matching memories via `recall_from_namespace(query, limit, ...)`
2. If empty → return error "no matching memories"
3. Call `self.add_to_namespace(new_content, ...)` → get `new_id`
4. Collect all matching IDs
5. Call `self.storage.supersede_bulk(&ids, &new_id)`
6. Return `BulkCorrectionResult`

The confirmation step (mentioned in GOAL-ss.5) is handled at the CLI layer, not the library. The library method is unconditional — the CLI shows matches and prompts before calling it.

**Note on search semantics:** `correct_bulk` uses `recall_from_namespace()` which applies the full 6-channel scoring pipeline (embedding + FTS + ACT-R + entity + temporal + Hebbian). Results are ranked by *relevance*, not exact text match — a high-importance memory with moderate text similarity may rank above an exact-match memory with low importance. The `limit` parameter caps results, so if there are 100 wrong memories but `limit=50`, only the top 50 by relevance get superseded. For very large correction sets, multiple passes may be needed.

### §2.4 Auto-Detection (GOAL-ss.6, P2)

**When:** During `Memory::add_to_namespace()`, after the new memory is stored.

**Detection logic:**

```rust
pub struct SupersessionCandidate {
    pub old_id: String,
    pub old_content: String,
    pub similarity: f32,
    pub reason: String, // e.g., "high similarity + negation detected"
}

pub struct StoreResult {
    pub id: String,
    pub supersession_candidates: Vec<SupersessionCandidate>,
}
```

Steps:
1. If no embedding provider → skip, return empty candidates
2. Embed the new content
3. Find top-5 most similar existing memories (cosine similarity > configured threshold, default 0.85)
4. For each high-similarity match, check for negation signals between old and new content
5. Negation detection: simple heuristic — check if new content contains negation tokens ("not", "doesn't", "no longer", "isn't", "不", "没有", "不再", "并非") that are absent from the old content, OR if new content explicitly uses correction markers ("actually", "correction", "其实", "更正")
6. If both similarity AND negation → add to candidates
7. Return candidates (caller decides whether to supersede)

**Note on language coverage:** Default negation tokens and correction markers cover English and Chinese only. Users of other languages should configure custom tokens via `SupersessionConfig`. The heuristic is intentionally simple — for cross-language or complex semantic contradiction detection, use the explicit `correct()` API instead.

**Configuration:**

```rust
pub struct SupersessionConfig {
    /// Enable auto-detection during store (default: false)
    pub auto_detect: bool,
    /// Minimum embedding similarity for candidate detection (default: 0.85)
    pub similarity_threshold: f32,
    /// Negation tokens to check (configurable, has defaults)
    pub negation_tokens: Vec<String>,
    /// Correction markers (configurable, has defaults)
    pub correction_markers: Vec<String>,
}
```

**Note on return type change:** Currently `Memory::add()` returns `Result<String>` (just the ID). To return candidates, either:
- (a) Change return type to `Result<StoreResult>` — **breaking change**
- (b) Add a separate `Memory::add_with_detection()` method — **non-breaking**

**Decision: Option (b).** `add()` stays unchanged. New `add_with_detection()` returns `StoreResult`. This satisfies GUARD-ss.2 (backward compatibility). The CLI `store` command can optionally use the detection variant.

**P2 boundary:** `StoreResult`, `SupersessionCandidate`, `SupersessionConfig`, and `add_with_detection()` are all P2-only. They are NOT included in the P0/P1 implementation. The types are defined here for completeness but should only be implemented when auto-detection ships.

### §2.5 Observability (OBS-ss.1)

**`Storage::list_superseded()`** returns all memories where `superseded_by != ''`, with their replacement ID. The `Memory` layer wraps this with chain resolution:

```rust
pub fn list_superseded(
    &self,
    namespace: Option<&str>,
) -> Result<Vec<SupersessionInfo>, Box<dyn std::error::Error>>

pub struct SupersessionInfo {
    pub superseded: MemoryRecord,
    pub superseded_by_id: String,
    pub chain_head: Option<String>, // final non-superseded memory (None if cycle)
}
```

The CLI `superseded` command prints a table of superseded memories with their replacement chain.

---

## §3 Guard Satisfaction

| Guard | How |
|-------|-----|
| GUARD-ss.1 (No deletion) | `supersede()` only sets `superseded_by` column. No `DELETE` statements. Old memory stays in DB. |
| GUARD-ss.2 (Backward compat) | `contradicted_by` field untouched. ACT-R penalty unchanged. New `superseded_by` is parallel field. `add()` return type unchanged (new method for detection). |
| GUARD-ss.3 (Safe migration) | `ALTER TABLE ADD COLUMN ... DEFAULT ''`. Idempotent via try-catch on "duplicate column" error. No re-indexing. |
| GUARD-ss.4 (Performance <1ms) | SQL `WHERE` clause on text column — SQLite evaluates `= ''` with O(1) string compare per row. No index needed (empty string check is fast). For 100K rows, ~0.1ms overhead. |

---

## §4 CLI Commands

Three new subcommands added to `enum Commands`:

### `engram correct <old_id> "new content"`

```rust
Correct {
    /// ID of the memory to correct
    old_id: String,
    /// Corrected content
    content: String,
    /// Namespace
    #[arg(long, short = 'n', default_value = "default")]
    ns: String,
    /// Override importance
    #[arg(long, short = 'i')]
    importance: Option<f64>,
    /// Override memory type
    #[arg(long, short = 't')]
    r#type: Option<MemoryTypeArg>,  // Reuses existing MemoryTypeArg from Store command
}
```

Calls `memory.correct(old_id, content, importance, type)`. Prints new ID and confirmation.

### `engram correct-bulk --query "wrong fact" "new content"`

```rust
CorrectBulk {
    /// Corrected content
    content: String,
    /// Query to find memories to supersede
    #[arg(long, short = 'q')]
    query: String,
    /// Namespace
    #[arg(long, short = 'n', default_value = "default")]
    ns: String,
    /// Max memories to search
    #[arg(long, short = 'l', default_value = "50")]
    limit: usize,
    /// Skip confirmation prompt
    #[arg(long, short = 'y')]
    yes: bool,
}
```

Flow:
1. Search for matching memories
2. Print matches (ID, content preview, created_at)
3. If `--yes` not set → prompt "Supersede N memories? [y/N]"
4. Call `memory.correct_bulk(query, content, ns, limit)`
5. Print result count

### `engram superseded`

```rust
Superseded {
    /// Namespace filter
    #[arg(long, short = 'n')]
    ns: Option<String>,
    /// Output as JSON
    #[arg(long, short = 'j')]
    json: bool,
}
```

Lists all superseded memories. Prints: ID, content (truncated), superseded_by, chain_head.

### Auto-detection output (P2)

When `store` uses `add_with_detection()` and candidates are found, CLI prints:

```
⚠️  Possible supersession candidates detected:
  [1] abc123 (0.92 similarity): "engram uses MCP" — high similarity + negation detected
  [2] def456 (0.87 similarity): "MCP is the transport layer" — high similarity + correction marker

Supersede these memories? [y/N/select]:
```

This is P2 — not implemented in the initial release.

---

## §5 Data Flow

### Normal Recall (with supersession filter)

```
User query
    ↓
Memory::recall_from_namespace()
    ↓
┌─ Storage::search_fts_ns()  ──→ SQL: WHERE ... AND superseded_by = '' 
│  Storage::get_embeddings()  ──→ (embeddings table, no supersession column)
│  Storage::get(id)           ──→ fetch candidate by ID (no filter)
└─ → candidate set
    ↓
Memory: candidates.retain(|r| r.superseded_by.is_none())  ← safety net
    ↓
6-channel scoring (FTS + embedding + ACT-R + entity + temporal + Hebbian)
    ↓
Sorted results (no superseded memories present)
```

### Other Recall Paths (same dual-filter pattern)

```
Memory::recall_recent()         → Storage::fetch_recent()  [SQL filter] → .retain() → return
Memory::hybrid_recall()         → hybrid_search::hybrid_search()  [SQL filter] → .retain() → return
Memory::recall_associated_ns()  → Storage::search_by_type_ns()  [SQL filter] → .retain() → return
Memory::recall_with_associations() → recall_from_namespace() [already filtered] → .retain() → return
```

### Correction Flow

```
engram correct <old_id> "new content"
    ↓
Memory::correct(old_id, new_content)
    ├── Storage::get(old_id) → validate exists, get metadata
    ├── Memory::add_to_namespace(new_content, ...) → new_id
    └── Storage::supersede(old_id, new_id)
            ├── validate both IDs exist
            ├── validate same namespace
            └── UPDATE memories SET superseded_by = new_id WHERE id = old_id
```

### Bulk Correction Flow

```
engram correct-bulk --query "wrong fact" "new content"
    ↓
CLI: Memory::recall_from_namespace("wrong fact", 50) → matches
CLI: print matches, prompt confirmation
    ↓
Memory::correct_bulk("wrong fact", "new content")
    ├── Memory::add_to_namespace("new content") → new_id
    └── Storage::supersede_bulk(match_ids, new_id)
            ├── SAVEPOINT
            ├── validate all IDs
            ├── UPDATE ... for each
            └── RELEASE SAVEPOINT
```

---

## §6 MemoryRecord Changes

Add one field to `MemoryRecord` (in `types.rs`):

```rust
pub struct MemoryRecord {
    // ... existing fields ...
    
    /// Contradiction links (legacy, penalty-based)
    pub contradicts: Option<String>,
    pub contradicted_by: Option<String>,
    
    /// Supersession link (new, filter-based).
    /// If set, this memory is excluded from all recall results.
    /// Contains the ID of the memory that replaced this one.
    pub superseded_by: Option<String>,
    
    // ... rest ...
}
```

Every place that constructs a `MemoryRecord` with hardcoded fields needs `superseded_by: None` added. There are **17 sites** across:

- `association/candidate.rs` (1)
- `association/former.rs` (1)
- `promotion.rs` (1)
- `memory.rs` (6 — `add_to_namespace`, test helpers, etc.)
- `models/actr.rs` (1)
- `synthesis/provenance.rs` (1)
- `synthesis/gate.rs` (1)
- `synthesis/insight.rs` (1)
- `synthesis/cluster.rs` (1)
- `synthesis/engine.rs` (1)
- `storage.rs` (2 — `row_to_record` + test helpers)

This is mechanical — same pattern as existing `contradicted_by: None`. Missing any site will cause a compile error since `MemoryRecord` has no `Default` impl and `superseded_by` has no default value.

---

## §7 Test Plan

| Test | Validates |
|------|-----------|
| `test_supersede_basic` | supersede(A, B) → recall excludes A, includes B |
| `test_supersede_self_error` | supersede(A, A) → error |
| `test_supersede_not_found` | supersede(nonexistent, B) → error |
| `test_supersede_cross_namespace` | supersede across namespaces → error |
| `test_supersede_already_superseded` | supersede(A, B) then supersede(A, C) → A.superseded_by = C |
| `test_recall_prefilter` | 10 memories, supersede 5, recall → only 5 returned |
| `test_recall_fts_prefilter` | same but via FTS path |
| `test_recall_recent_prefilter` | superseded memories excluded from recall_recent |
| `test_hybrid_recall_prefilter` | superseded memories excluded from hybrid_recall |
| `test_recall_associated_prefilter` | superseded memories excluded from recall_associated_ns |
| `test_correct_single` | correct(old, "new") → old superseded, new exists |
| `test_correct_inherits_metadata` | new memory has old's memory_type and namespace |
| `test_bulk_supersede_transactional` | one invalid ID → all rolled back |
| `test_bulk_supersede_empty` | empty list → success, count 0 |
| `test_correct_bulk` | correct_bulk with query → matches superseded |
| `test_chain_resolution` | A→B→C, recall → only C |
| `test_chain_cycle_detection` | A→B→A, resolve_chain_head → None |
| `test_unsupersede` | supersede then unsupersede → memory active again |
| `test_unsupersede_chain_partial` | A→B→C, unsupersede(B) → B active, A still superseded |
| `test_list_superseded` | list shows all superseded with chain heads |
| `test_schema_migration_idempotent` | open DB twice → no error |
| `test_backward_compat_contradicted_by` | contradicted_by still applies penalty after supersession feature added |

---

## §8 Implementation Order

1. **Schema + types** — Add `superseded_by` column migration, update `MemoryRecord`, update `row_to_record`
2. **Storage CRUD** — `supersede()`, `supersede_bulk()`, `unsupersede()`, `list_superseded()`, `resolve_chain_head()`
3. **SQL filter** — Add `AND superseded_by = ''` to all recall SQL queries
4. **Memory safety net** — `candidates.retain()` in recall paths
5. **Memory::correct()** and `Memory::correct_bulk()`
6. **CLI commands** — `correct`, `correct-bulk`, `superseded`
7. **Auto-detection** — `add_with_detection()`, `SupersessionConfig` (P2, can be separate PR)
8. **Tests** — unit tests for each component
