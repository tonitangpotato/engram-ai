# Design Review: FEAT-003 Memory Lifecycle
> Reviewed: 2026-04-17
> Target: `.gid/features/memory-lifecycle/design.md`
> Depth: standard (Phase 0-5)
> **Status: All findings applied 2026-04-17**

## Summary

9 findings: **3 critical**, **4 important**, **2 minor**. **All applied.**

The design covers all 9 components comprehensively and the overall architecture is sound. However, several API signatures and table references didn't match the actual codebase. The biggest structural issue was C5's decay model conflating two different concepts of "stability" (stored column vs computed value).

---

## Findings

### FINDING-1 ✅ Applied — C1: `add_to_namespace()` signature mismatch

Design said modify `add_to_namespace()` but dedup actually lives in `add_raw()`.

**Applied**: Rewrote C1 to modify `add_raw()` instead. Fixed signature to match actual params (Option<f64> importance, source, metadata, Option<&str> namespace). Preserved extractor→add_raw pipeline.

---

### FINDING-2 ✅ Applied — C5: Decay model is circular / conflates stability

Design added a `stability` column but `ebbinghaus.rs` already has `compute_stability()` that derives stability from access_times, importance, consolidation_count. Two competing models would exist.

**Applied**: Chose Option B — removed `stability` column from migration, removed `boost_stability()` and `decay_tick()`. Rewrote C5 to use existing `compute_stability()` + `retrievability()` + `effective_strength()` pipeline. Recall naturally improves stability via `record_access()` adding timestamps. Also updated C7 sleep_cycle to use `decay_check` (reads existing model) instead of `decay_tick` (writes stored column).

---

### FINDING-3 ✅ Applied — C6: `cluster_members` table doesn't exist

Design referenced nonexistent tables in `hard_delete_cascade()`.

**Applied**: Fixed all table names:
- `embeddings` → `memory_embeddings_v2`
- `entity_links` → `memory_entities`
- `cluster_members` → `synthesis_provenance`

---

### FINDING-4 ✅ Applied — C2: `merge_memory_into()` already does provenance

Design claimed "Gap: no provenance chain" but merge_history already existed in metadata.

**Applied**: Updated C2 status to acknowledge existing `merge_history` (ts, sim, content_updated, capped at 10). Framed as enhancement: "merge_history exists but lacks source_id. Enhance format to include source_id field."

---

### FINDING-5 ✅ Applied — C8: Co-occurrence misuses `normalize_entity_name` on memory IDs

Design called `normalize_entity_name(prev_id)` where prev_id is a UUID — a no-op.

**Applied**: Rewrote C8 to use existing `record_coactivation_ns()` instead of reimplementing Hebbian link creation. Moved GOAL-18 entity normalization to `extract_entities()` pipeline (where it belongs). Removed `normalize_entity_name` calls from recall().

---

### FINDING-6 ✅ Applied — C7: `sleep_cycle()` signature mismatch

Design used `fn sleep_cycle(&mut self)` but actual has `days: f64, namespace: Option<&str>`.

**Applied**: Fixed signature to `fn sleep_cycle(&mut self, days: f64, namespace: Option<&str>)`. New phases added inside existing signature.

---

### FINDING-7 ✅ Applied — C8: `recall()` signature mismatch + mutability

Design had wrong params and return type.

**Applied**: Fixed to `fn recall(&mut self, query: &str, limit: usize, context: Option<Vec<String>>, min_confidence: Option<f64>) -> Result<Vec<RecallResult>, LifecycleError>`. Noted namespace uses separate `recall_from_namespace` method.

---

### FINDING-8 ✅ Applied — C3: O(n²) reconcile without bound

No upper bound on pairwise comparison.

**Applied**: Added `max_scan: Option<usize>` param (default 1000) to `reconcile()`. Embeddings loaded bounded by max_scan, sorted by created_at DESC (newest first).

---

### FINDING-9 ✅ Applied — LifecycleError defined twice with different variants

Two definitions with different variants in §1 and §3.

**Applied**: Removed inline definition from §1, added reference to §3 Cross-Cutting Concerns as canonical location. §3 version has complete variants including `EmbeddingUnavailable(String)`, `Synthesis`, `Consolidation`.

---

## Verdict

**✅ All 9 findings applied.** Design is now aligned with actual codebase signatures and ready for implementation.
