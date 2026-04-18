# Design Review R2: FEAT-003 Memory Lifecycle
> Reviewed: 2026-04-17 (second pass after applying R1 findings)
> Target: `.gid/features/memory-lifecycle/design.md`
> Depth: standard (Phase 0-5)

## Summary

7 findings: **3 critical**, **3 important**, **1 minor**.

R1 fixed the major structural issues (add_raw location, stability column, table names). This pass caught **signature-level mismatches** that survived R1 — the design still has several API calls that don't match actual code. These would cause compile errors.

---

## Findings

### FINDING-1 ⛔ Critical — C5: `effective_strength()` and `compute_stability()` signatures wrong ✅ Applied

Design C5 calls:
```rust
let stability = ebbinghaus::compute_stability(
    &record.access_times,
    record.importance,
    record.consolidation_count,
);
let retrievability = ebbinghaus::retrievability(record, now);
let effective = ebbinghaus::effective_strength(stability, retrievability);
```

Actual signatures (from `src/models/ebbinghaus.rs`):
```rust
pub fn compute_stability(record: &MemoryRecord) -> f64     // takes &MemoryRecord, not individual fields
pub fn retrievability(record: &MemoryRecord, now: DateTime<Utc>) -> f64
pub fn effective_strength(record: &MemoryRecord, now: DateTime<Utc>) -> f64   // takes &MemoryRecord, not (stability, retrievability)
```

Also: design references `record.access_count` but `MemoryRecord` has no `access_count` field. The correct equivalent is `record.access_times.len()`.

**Fix**: Replace C5's computation with:
```rust
let effective = ebbinghaus::effective_strength(record, now);
if effective < 0.1 && record.access_times.len() < 2 {
    self.storage.soft_delete(&record.id)?;
}
```
No need to call `compute_stability` or `retrievability` individually — `effective_strength` wraps them internally.

**Applied**: Replaced C5's three separate ebbinghaus calls with single `effective_strength(record, now)`. Fixed `record.access_count` → `record.access_times.len()`. Updated C5 status text to document correct signatures.

---

### FINDING-2 ⛔ Critical — C8: `record_coactivation_ns()` called with wrong params ✅ Applied

Design C8 calls:
```rust
self.storage.record_coactivation_ns(
    prev_id,
    &record.id,
    0.05,  // weight increment per co-occurrence
)?;
```

Actual signature (`storage.rs:1696`):
```rust
pub fn record_coactivation_ns(
    &mut self,
    id1: &str,
    id2: &str,
    threshold: i32,    // NOT f64! Integer coactivation count threshold before forming link
    namespace: &str,   // MISSING from design call
) -> Result<bool, rusqlite::Error>
```

Three errors:
1. `0.05` is a `f64` but param is `i32` — would truncate to 0, meaning links form immediately (unintended)
2. Missing 4th param `namespace: &str`
3. The comment says "weight increment" but it's actually "coactivation count threshold before forming a Hebbian link"

**Fix**: Use the coactivation threshold from config (probably 3) and pass namespace:
```rust
let ns = namespace.unwrap_or("default");
self.storage.record_coactivation_ns(
    prev_id,
    &record.id,
    self.config.hebbian_threshold,  // e.g. 3
    ns,
)?;
```

Note: there's also a **free function** `models::hebbian::record_coactivation_ns(storage, memory_ids, threshold, namespace)` that takes `&[String]` — could use that for batch co-occurrence instead of per-pair calls.

**Applied**: Fixed `record_coactivation_ns` call in C8 — changed `0.05` (f64) to `self.config.hebbian_threshold` (i32), added missing `ns` namespace param, fixed comment to say "coactivation count before link forms" instead of "weight increment".

---

### FINDING-3 ⛔ Critical — C8: `recall()` calls nonexistent `self.storage.hybrid_search()` ✅ Applied

Design C8's recall says:
```rust
let results = self.storage.hybrid_search(query, limit, context.as_deref(), min_confidence)?;
```

But:
1. `hybrid_search` is a **free function** in `hybrid_search.rs`, NOT a method on Storage: `pub fn hybrid_search(storage: &Storage, query_vector: Option<&[f32]>, query_text: &str, opts: HybridSearchOpts, model: &str)`
2. The actual `recall()` doesn't even use `hybrid_search()` — it has its own inline implementation (embedding similarity + FTS + entity recall + ACT-R boosting, all combined in the method body)
3. The design changes recall's return type from `Result<Vec<RecallResult>, Box<dyn std::error::Error>>` to `Result<Vec<RecallResult>, LifecycleError>` — this changes the existing error type contract

**Fix**: C8's co-occurrence logic should be **added into the existing recall implementation**, not replace it. Remove the `hybrid_search` line. The co-occurrence tracking (recent_recalls ring buffer + record_coactivation_ns) is additive — insert it after the existing recall logic returns results:

```rust
// At end of existing recall_from_namespace(), before return:
// Co-occurrence detection (GOAL-17)
let now_instant = Instant::now();
for record in &results {
    for (prev_id, prev_time) in &self.recent_recalls {
        if prev_id == &record.record.id { continue; }
        if now_instant.duration_since(*prev_time) <= Duration::from_secs(30) {
            let _ = self.storage.record_coactivation_ns(
                prev_id, &record.record.id,
                self.config.hebbian_threshold, ns,
            );
        }
    }
    self.recent_recalls.push_back((record.record.id.clone(), now_instant));
    if self.recent_recalls.len() > 50 { self.recent_recalls.pop_front(); }
}
```

Keep existing return type `Box<dyn std::error::Error>` — don't change to `LifecycleError` for existing methods.

**Applied**: Replaced C8's full `recall()` method (which called nonexistent `self.storage.hybrid_search()` and changed return type to `LifecycleError`) with an additive code block to inject at end of existing `recall_from_namespace()`. Preserved existing return type `Box<dyn std::error::Error>`. Uses `result.record.id` (correct RecallResult field access) and `let _ =` for non-fatal coactivation recording.

---

### FINDING-4 ⚠️ Important — C6: `memory_embeddings_v2` is wrong table name ✅ Applied

R1 changed `embeddings` → `memory_embeddings_v2` in `hard_delete_cascade`. But the migration code (`storage.rs:556-557`) does:
```sql
DROP TABLE memory_embeddings;
ALTER TABLE memory_embeddings_v2 RENAME TO memory_embeddings;
```

After migration, the table is **`memory_embeddings`** (v2 gets renamed). All live queries use `memory_embeddings`. The cascade delete should use `memory_embeddings`:

```rust
self.conn.execute("DELETE FROM memory_embeddings WHERE memory_id = ?1", [id])?;
```

**Applied**: Changed `memory_embeddings_v2` → `memory_embeddings` in C6's `hard_delete_cascade()` method to match the actual table name after migration.

---

### FINDING-5 ⚠️ Important — C1: `add_raw()` return type change breaks callers ✅ Applied

Design changes `add_raw()` from `Result<String, Box<dyn Error>>` to `Result<AddResult, LifecycleError>`.

Current callers in `add_to_namespace()` (line 523, 549):
```rust
last_id = self.add_raw(content, memory_type, importance, source, metadata, namespace)?;
// ...
self.add_raw(content, memory_type, importance, source, metadata, namespace)
```

These expect `String` (the memory ID). Changing to `AddResult` breaks both callsites + `add()` which wraps `add_to_namespace()` and also returns `String`.

**Fix**: Either (a) keep returning `String` and add dedup info via a separate `last_add_result` field on Memory, or (b) change `AddResult` to implement `Into<String>` and update all callers. Option (a) is less invasive:
```rust
fn add_raw(...) -> Result<String, Box<dyn std::error::Error>> {
    // ... dedup logic ...
    // Store last result for metrics
    self.last_add_result = Some(AddResult::Merged { into: id.clone(), similarity });
    Ok(id)
}
```

**Applied**: Changed C1's `add_raw()` return type back to `Result<String, Box<dyn std::error::Error>>`. Added `last_add_result: Option<AddResult>` field to `Memory` struct. Each code path now stores the `AddResult` on `self.last_add_result` for metrics and returns `Ok(id)` / `Ok(existing_id)` / `Ok(candidate_id)` as `String`.

---

### FINDING-6 ⚠️ Important — C8: `normalize_entity_name()` signature mismatch ✅ Applied

Design redefines:
```rust
pub fn normalize_entity_name(name: &str) -> String {
    name.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}
```

Actual signature (`entities.rs:272`):
```rust
pub fn normalize_entity_name(name: &str, entity_type: &EntityType) -> String
```

Takes `entity_type` as second param and applies type-specific normalization (Person → strip @, URL → strip trailing /, etc.). Removing the param would break all callers and lose type-specific logic.

**Fix**: Keep existing signature. If GOAL-18 needs additional whitespace normalization, add it to the existing function body:
```rust
pub fn normalize_entity_name(name: &str, entity_type: &EntityType) -> String {
    let mut normalized = name.trim().to_lowercase();
    // Collapse whitespace (GOAL-18 enhancement)
    normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    match entity_type { /* existing type-specific logic */ }
    normalized
}
```

**Applied**: Updated C8's `normalize_entity_name` to use correct 2-param signature `(name: &str, entity_type: &EntityType)`, preserving type-specific normalization logic while adding GOAL-18 whitespace collapse.

---

### FINDING-7 📝 Minor — C9: `avg_stability()` helper needs clarification ✅ Applied

`HealthReport` has `avg_stability: f64` and calls `self.storage.avg_stability()`. But stability is computed from `MemoryRecord` fields (access_times, importance, consolidation_count) — it's NOT stored in a column. This helper would need to:
1. Load ALL MemoryRecords from the namespace
2. Call `ebbinghaus::compute_stability(record)` for each
3. Average the results

Same for `count_below_stability(0.1)`. These are O(N) full-table scans with computation. Should document this cost and consider caching or sampling for large databases.

**Fix**: Add comment that these are computed values requiring full scan:
```rust
/// Compute average stability across all memories.
/// NOTE: O(N) — loads all records and computes stability from Ebbinghaus model.
/// For large DBs, consider sampling.
pub fn avg_stability(&self) -> Result<f64, rusqlite::Error>
```

**Applied**: Added O(N) cost documentation and sampling recommendation to `avg_stability()` and `count_below_stability()` signatures in C9's storage helpers section.

---

## Verdict

**Needs one more pass.** FINDING-1,2,3 would cause compile errors. FINDING-4 would cascade-delete from a table that no longer exists (renamed during migration). These are the same class of issue as R1 — API calls not matching actual code — just in different locations.

The design architecture is sound. The remaining issues are all "check the actual function signature" fixes, not structural problems.
