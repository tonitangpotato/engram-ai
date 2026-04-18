# FEAT-003: Memory Lifecycle — Design Document

> Feature: Memory Lifecycle Management
> Date: 2026-04-17
> Status: Draft
> Crate: engramai

## 1. Overview

This design covers 9 components that complete the memory lifecycle in engramai:
write-time dedup → merge → reconcile → incremental synthesis → decay → forget → sleep orchestration → co-occurrence → health/rebalance.

**Guiding principle**: extend existing APIs where possible (add_to_namespace, merge_memory_into, sleep_cycle, forget). New APIs only for genuinely new capabilities (reconcile, health, rebalance).

**New DB columns** (migration):
- `memories.deleted_at` TEXT DEFAULT NULL

**New error type**: See §3 Cross-Cutting Concerns → Error Types for the canonical `LifecycleError` definition.

---

## 2. Component Designs

### C1: Semantic Deduplication (On-Write)

**Status**: ~60% done. `add_raw()` already calls `find_nearest_embedding(0.85)` and merges on match. The extractor→`add_raw()` pipeline must be preserved (`add_to_namespace()` routes through the extractor first). Gap: no entity Jaccard check, no `AddResult` return type, no dedup metrics.

**Changes to `memory.rs`**:

```rust
/// Result of an add operation after dedup checks.
pub enum AddResult {
    /// New memory created.
    Created { id: String },
    /// Merged into existing memory.
    Merged { into: String, similarity: f32 },
}
```

**Modified method** — enhance `add_raw()` with dedup (preserve existing signature + pipeline).

**Return type**: `add_raw()` continues to return `Result<String, Box<dyn std::error::Error>>` to preserve compatibility with existing callers in `add_to_namespace()`. The `AddResult` enum is stored on `self.last_add_result` for metrics/informational purposes only.

**New field on `Memory` struct**:
```rust
/// Last add result for metrics access. Reset each sleep_cycle.
last_add_result: Option<AddResult>,
```

```rust
pub fn add_raw(
    &mut self,
    content: &str,
    memory_type: MemoryType,
    importance: Option<f64>,
    source: Option<&str>,
    metadata: Option<serde_json::Value>,
    namespace: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    let entities = self.extract_entities(content);
    let embedding = self.embed(content)?;
    let ns = namespace.unwrap_or("default");

    // Phase 1: entity Jaccard check against recent memories
    // (skip if no entities extracted — fall through to embedding-only)
    if !entities.is_empty() {
        if let Some((candidate_id, jaccard)) =
            self.storage.find_entity_overlap(&entities, ns, 0.5)?
        {
            // Jaccard ≥ 0.5 AND embedding ≥ 0.85 → merge
            if let Some(emb) = embedding.as_ref() {
                if let Some((_, cosine)) =
                    self.storage.find_nearest_embedding(emb, &self.config.embedding_model, Some(ns), 0.85)?
                {
                    self.storage.merge_memory_into(&candidate_id, content, importance, cosine)?;
                    self.last_add_result = Some(AddResult::Merged { into: candidate_id.clone(), similarity: cosine });
                    self.dedup_merge_count += 1;
                    return Ok(candidate_id);  // Returns String, not AddResult
                }
            }
        }
    }

    // Phase 2: embedding-only check (existing logic)
    if let Some(emb) = embedding.as_ref() {
        if let Some((existing_id, score)) =
            self.storage.find_nearest_embedding(emb, &self.config.embedding_model, Some(ns), 0.85)?
        {
            self.storage.merge_memory_into(&existing_id, content, importance, score)?;
            self.last_add_result = Some(AddResult::Merged { into: existing_id.clone(), similarity: score });
            self.dedup_merge_count += 1;
            return Ok(existing_id);  // Returns String, not AddResult
        }
    }

    // Phase 3: no match → create new
    let id = self.storage.create_memory(content, memory_type, importance, source, metadata, namespace, embedding.as_deref())?;
    self.last_add_result = Some(AddResult::Created { id: id.clone() });
    self.dedup_write_count += 1;
    Ok(id)  // Returns String, not AddResult
}
```

**New storage method** — `find_entity_overlap()`:

```rust
// storage.rs
pub fn find_entity_overlap(
    &self,
    entities: &[EntityRecord],
    namespace: &str,
    threshold: f64,  // Jaccard threshold, e.g. 0.5
) -> Result<Option<(String, f64)>, rusqlite::Error> {
    // Query entity_links table for memories sharing entities in this namespace
    // Group by memory_id, compute |intersection| / |union| as Jaccard
    // Return best match above threshold
    // SQL: SELECT memory_id, COUNT(DISTINCT entity) as overlap
    //      FROM entity_links WHERE entity IN (?...) AND namespace = ?
    //      GROUP BY memory_id ORDER BY overlap DESC LIMIT 10
    // Then compute Jaccard = overlap / (input_count + target_count - overlap)
}
```

**Metrics** — add to `Memory` struct:
```rust
// Counters reset each sleep_cycle, reported in SleepReport
dedup_merge_count: usize,
dedup_write_count: usize,
// dedup_merge_rate = dedup_merge_count / (dedup_merge_count + dedup_write_count)
```

---

### C2: Intelligent Merge (Enhanced)

**Status**: ~80% done. `merge_memory_into()` handles access_count bump, timestamp preservation, content append if >30% longer. It already stores `merge_history` in metadata (with `ts`, `sim`, `content_updated`, capped at 10). Gap: no Hebbian link union, merge_history lacks `source_id` field for provenance tracing.

**Changes to `storage.rs::merge_memory_into()`**:

After the existing merge logic, add two steps:

```rust
// Step A: Hebbian link union (GOAL-4)
// Collect all hebbian_links where source=donor_id OR target=donor_id
// For each link:
//   - Repoint donor_id → target_id (the merge survivor)
//   - If link already exists on target, take max(weight), sum(co_occurrences)
//   - If self-link after repoint (source==target), drop it
pub fn merge_hebbian_links(
    &self,
    donor_id: &str,
    target_id: &str,
) -> Result<usize, rusqlite::Error> {
    let donor_links = self.get_hebbian_links(donor_id)?;
    let mut transferred = 0;
    for link in &donor_links {
        let other = if link.source_id == donor_id { &link.target_id } else { &link.source_id };
        if other == target_id { continue; } // skip self-links
        // upsert_hebbian_link already does INSERT OR UPDATE with max weight
        self.upsert_hebbian_link(target_id, other, link.weight)?;
        transferred += 1;
    }
    // Clean up donor's links
    self.delete_hebbian_links_for(donor_id)?;
    Ok(transferred)
}
```

```rust
// Step B: Enhanced provenance chain (GOAL-5 / GOAL-24a)
// merge_history already exists in metadata (ts, sim, content_updated, capped at 10).
// Enhancement: add source_id field to each entry for full provenance tracing.
pub struct MergeHistoryEntry {
    pub source_id: String,          // NEW: donor memory ID
    pub timestamp: DateTime<Utc>,   // existing as "ts"
    pub similarity: f32,            // existing as "sim"
    pub content_updated: bool,      // existing field preserved
}
// Implementation: read target metadata JSON, parse merge_history vec,
// push new entry (now including source_id), truncate to 10 (FIFO), write back.
// In merge_memory_into(), enhance existing provenance write:
fn append_merge_provenance(
    &self,
    target_id: &str,
    source_id: &str,
    similarity: f32,
    content_updated: bool,
) -> Result<(), rusqlite::Error> {
    let mut meta = self.get_memory_metadata(target_id)?; // serde_json::Value
    let history = meta.as_object_mut()
        .and_then(|m| m.entry("merge_history").or_insert(json!([])).as_array_mut());
    if let Some(arr) = history {
        arr.push(json!({
            "source_id": source_id,
            "ts": Utc::now().to_rfc3339(),
            "sim": similarity,
            "content_updated": content_updated
        }));
        // Cap at 10 entries (FIFO)
        while arr.len() > 10 { arr.remove(0); }
    }
    self.update_memory_metadata(target_id, &meta)?;
    Ok(())
}
```

**Updated `merge_memory_into()` flow**:
1. (existing) Content merge, importance max, access_count sum, timestamps
2. (new) `merge_hebbian_links(donor_id, target_id)`
3. (new) `append_merge_provenance(target_id, donor_id, similarity)`
4. (existing) Delete donor memory

---

### C3: Reconcile API

**Status**: New. No existing code.

**New methods in `memory.rs`**:

```rust
/// A candidate merge pair found by reconcile scan.
pub struct ReconcileCandidate {
    pub id_a: String,
    pub id_b: String,
    pub similarity: f32,
    pub entity_overlap: f64,  // Jaccard score
    pub content_preview_a: String,  // first 100 chars
    pub content_preview_b: String,
}

/// Result of reconcile_apply.
pub struct ReconcileReport {
    pub scanned: usize,
    pub candidates_found: usize,
    pub merges_applied: usize,
    pub dry_run: bool,
}

/// Scan namespace for duplicate pairs. (GOAL-6)
/// `max_scan` bounds the pairwise comparison to avoid O(n²) blowup on large namespaces.
/// Default: 1000. Memories are sorted by created_at DESC (newest first) before scanning.
pub fn reconcile(
    &self,
    namespace: &str,
    permission: Permission,
    max_scan: Option<usize>,  // default 1000
) -> Result<Vec<ReconcileCandidate>, LifecycleError> {
    // GUARD: admin only (GOAL-8)
    if !permission.is_admin() {
        return Err(LifecycleError::PermissionDenied {
            op: "reconcile", required: Permission::Admin
        });
    }

    let max_scan = max_scan.unwrap_or(1000);

    // 1. Load embeddings in namespace (bounded by max_scan, sorted by created_at DESC)
    let embeddings = self.storage.get_embeddings_in_namespace(
        Some(namespace), &self.config.embedding_model, max_scan
    )?;

    // 2. Pairwise cosine scan — bounded by max_scan parameter:
    //    For each memory, find_nearest_embedding against all others.
    //    Deduplicate pairs (only keep (min_id, max_id)).
    //    Threshold: 0.85 (same as GOAL-1)
    let mut candidates: Vec<ReconcileCandidate> = Vec::new();
    let mut seen_pairs: HashSet<(String, String)> = HashSet::new();

    for (id_a, emb_a) in &embeddings {
        // Use existing find_nearest but scan all, not just top-1
        let matches = self.storage.find_all_above_threshold(
            emb_a, &self.config.embedding_model, Some(namespace), 0.85
        )?;
        for (id_b, score) in matches {
            if id_a == &id_b { continue; }
            let pair = if id_a < &id_b { (id_a.clone(), id_b.clone()) }
                       else { (id_b.clone(), id_a.clone()) };
            if seen_pairs.contains(&pair) { continue; }
            seen_pairs.insert(pair);
            // Compute entity Jaccard for ranking
            let entities_a = self.storage.get_entities_for_memory(id_a)?;
            let entities_b = self.storage.get_entities_for_memory(&id_b)?;
            let jaccard = jaccard_similarity(&entities_a, &entities_b);
            candidates.push(ReconcileCandidate {
                id_a: id_a.clone(),
                id_b,
                similarity: score,
                entity_overlap: jaccard,
                content_preview_a: self.storage.get_memory_content_preview(id_a, 100)?,
                content_preview_b: self.storage.get_memory_content_preview(&id_b, 100)?,
            });
        }
    }

    // 3. Sort by combined score: 0.7 * similarity + 0.3 * entity_overlap
    candidates.sort_by(|a, b| {
        let score_a = 0.7 * a.similarity as f64 + 0.3 * a.entity_overlap;
        let score_b = 0.7 * b.similarity as f64 + 0.3 * b.entity_overlap;
        score_b.partial_cmp(&score_a).unwrap()
    });

    Ok(candidates)
}

/// Apply reconcile merges. (GOAL-7)
pub fn reconcile_apply(
    &mut self,
    candidates: &[ReconcileCandidate],
    dry_run: bool,
    permission: Permission,
) -> Result<ReconcileReport, LifecycleError> {
    if !permission.is_admin() {
        return Err(LifecycleError::PermissionDenied {
            op: "reconcile_apply", required: Permission::Admin
        });
    }

    let mut report = ReconcileReport {
        scanned: candidates.len(),
        candidates_found: candidates.len(),
        merges_applied: 0,
        dry_run,
    };

    if dry_run { return Ok(report); }

    // Track already-merged IDs to skip stale pairs
    let mut merged_away: HashSet<String> = HashSet::new();

    for candidate in candidates {
        if merged_away.contains(&candidate.id_a) || merged_away.contains(&candidate.id_b) {
            continue; // one side already merged into something else
        }
        // Keep the older memory (lower created_at), merge newer into older
        let (keep, donor) = self.pick_merge_target(&candidate.id_a, &candidate.id_b)?;
        self.storage.merge_memory_into(&keep, /* donor content */, /* importance */, candidate.similarity)?;
        self.storage.merge_hebbian_links(&donor, &keep)?;
        self.storage.append_merge_provenance(&keep, &donor, candidate.similarity)?;
        self.storage.delete_memory(&donor)?;
        merged_away.insert(donor);
        report.merges_applied += 1;
    }

    Ok(report)
}
```

**New storage helper** — `find_all_above_threshold()`:
```rust
// Like find_nearest_embedding but returns ALL matches above threshold, not just top-1
pub fn find_all_above_threshold(
    &self, embedding: &[f32], model: &str, namespace: Option<&str>, threshold: f64
) -> Result<Vec<(String, f32)>, rusqlite::Error>
```

---

### C4: Incremental Synthesis

**Status**: ~40% done. `IncrementalState` and `IncrementalConfig` exist in `synthesis/types.rs`. `SynthesisEngine::run()` exists but always does full synthesis. Gap: no staleness check, no seed reuse, no incremental counter.

**Design**: Modify `SynthesisEngine::run()` to check staleness before re-synthesizing each cluster.

```rust
// synthesis/engine.rs — new method
fn should_resynthesize(
    &self,
    cluster_id: usize,
    current_members: &HashSet<String>,
    state: &IncrementalState,
    config: &IncrementalConfig,
) -> bool {
    // Condition 1: member change > staleness_member_change_pct (default 0.5)
    let old_members = &state.last_member_snapshot;
    let intersection = current_members.intersection(old_members).count();
    let union = current_members.union(old_members).count();
    if union == 0 { return true; } // new cluster
    let change_pct = 1.0 - (intersection as f64 / union as f64);
    if change_pct >= config.staleness_member_change_pct { return true; }

    // Condition 2: quality_score delta > staleness_quality_delta (default 0.2)
    // quality_score computed from average member importance * coherence
    let current_quality = self.compute_cluster_quality(cluster_id, current_members);
    if (current_quality - state.last_quality_score).abs() >= config.staleness_quality_delta {
        return true;
    }

    false
}
```

**Modified `run()` flow**:
```rust
pub fn run(
    &mut self,
    storage: &mut Storage,
    config: &SynthesisConfig,
) -> Result<SynthesisReport, SynthesisError> {
    let clusters = self.cluster(storage, config)?;
    let mut report = SynthesisReport::default();

    for (cluster_id, members) in &clusters {
        let member_set: HashSet<String> = members.iter().cloned().collect();
        let state = self.get_incremental_state(*cluster_id);

        if let Some(state) = state {
            if !self.should_resynthesize(*cluster_id, &member_set, state, &config.incremental) {
                report.clusters_skipped += 1;
                continue;
            }
        }

        // Reuse existing InsightRecord as seed if available (GOAL-10)
        let seed = self.get_existing_insight(*cluster_id);
        let insight = self.synthesize_cluster(*cluster_id, members, seed.as_ref())?;

        // Update incremental state
        self.set_incremental_state(*cluster_id, IncrementalState {
            last_member_snapshot: member_set,
            last_quality_score: insight.quality_score,
            last_run: Utc::now(),
            run_count: state.map_or(1, |s| s.run_count + 1),
        });

        if seed.is_some() {
            report.synthesis_runs_incremental += 1; // GOAL-11
        } else {
            report.synthesis_runs_full += 1;
        }
        report.insights_generated += 1;
    }

    Ok(report)
}
```

**Seed reuse** — `synthesize_cluster()` with seed:
```rust
fn synthesize_cluster(
    &self,
    cluster_id: usize,
    members: &[String],
    seed: Option<&InsightRecord>,  // existing insight to refine
) -> Result<InsightRecord, SynthesisError> {
    // If seed exists, start from seed.content and adjust based on new/removed members
    // If no seed, synthesize from scratch (current behavior)
    // Quality score = average cosine similarity of members to insight embedding
}
```

**New counters in `SynthesisReport`**:
```rust
pub struct SynthesisReport {
    pub insights_generated: usize,
    pub clusters_processed: usize,
    pub clusters_skipped: usize,      // NEW: didn't need re-synthesis
    pub synthesis_runs_full: usize,    // NEW (GOAL-11)
    pub synthesis_runs_incremental: usize,  // NEW (GOAL-11)
    pub duration: Duration,
}
```

---

### C5: Decay Model

**Status**: `ebbinghaus.rs` has `compute_stability(record: &MemoryRecord)` that derives stability from the record's access_times, importance, and consolidation_count. It also has `retrievability(record, now)` and `should_forget()`. `effective_strength(record, now)` wraps both internally. consolidation.rs uses ACT-R base_level_activation. Both models are complementary.

**Design**: Use the existing Ebbinghaus model's `effective_strength(&MemoryRecord, now)` instead of adding a new `stability` column. Add GOAL-14's forget threshold into the existing `effective_strength` pipeline. No new DB columns needed for decay.

**Integration with existing Ebbinghaus model**:

```rust
/// Check memories for forget eligibility using existing Ebbinghaus model (GOAL-12, GOAL-14).
/// Called during sleep_cycle's forget phase.
/// Uses existing compute_stability() + retrievability() + should_forget().
fn check_decay_and_flag(
    &mut self,
    namespace: Option<&str>,
) -> Result<DecayReport, LifecycleError> {
    let memories = self.storage.get_all_memories_in_namespace(namespace)?;
    let now = Utc::now();
    let mut below_threshold = 0;
    let mut flagged_for_forget = 0;

    for record in &memories {
        // Use existing ebbinghaus model — effective_strength wraps
        // compute_stability + retrievability internally
        let effective = ebbinghaus::effective_strength(record, now);

        // GOAL-14: flag for soft-delete if effective_strength < 0.1 AND access_times.len() < 2
        if effective < 0.1 {
            below_threshold += 1;
            if record.access_times.len() < 2 {
                self.storage.soft_delete(&record.id)?;
                flagged_for_forget += 1;
                tracing::debug!(
                    memory_id = %record.id,
                    effective_strength = effective,
                    access_count = record.access_times.len(),
                    "memory flagged for forget via Ebbinghaus decay"
                );
            }
        }
    }

    Ok(DecayReport { below_threshold, flagged_for_forget })
}
```

**No new storage helpers needed** — uses existing `effective_strength(&MemoryRecord, now)` from `ebbinghaus.rs`, which internally calls `compute_stability(record)` and `retrievability(record, now)`. The existing `should_forget()` can also be used as an alternative entry point.

**Recall does NOT boost a stored stability value** — the existing model derives stability from access_times, which are already updated on recall via `record_access()`. Each recall naturally improves `compute_stability()` output by adding a new access timestamp.

```rust
pub struct DecayReport {
    pub below_threshold: usize,    // memories with effective_strength < 0.1
    pub flagged_for_forget: usize, // memories soft-deleted due to low strength + low access
}
```

---

### C6: Forget (Soft + Hard Delete)

**Status**: `forget()` exists but only does hard prune based on ACT-R activation. `delete_memory()` does hard delete but misses entity_links and synthesis cluster cleanup.

**Design**: Add `deleted_at` column; split forget into soft/hard modes.

**DB Migration**:
```sql
ALTER TABLE memories ADD COLUMN deleted_at TEXT DEFAULT NULL;
```

**Modified `forget()` in `memory.rs`**:

```rust
/// Forget memories. (GOAL-15, GOAL-16)
pub fn forget_targeted(
    &mut self,
    memory_id: &str,
    soft: bool,
    permission: Permission,
) -> Result<(), LifecycleError> {
    // GUARD-4: hard delete requires Admin
    if !soft && !permission.is_admin() {
        return Err(LifecycleError::PermissionDenied {
            op: "hard_delete", required: Permission::Admin
        });
    }

    if soft {
        // GOAL-15: set deleted_at, excluded from search
        self.storage.soft_delete(memory_id)?;
        tracing::info!(memory_id, "soft-deleted");
    } else {
        // GOAL-16 + GOAL-16a: cascade hard delete
        self.storage.hard_delete_cascade(memory_id)?;
        tracing::info!(memory_id, "hard-deleted with cascade");
    }
    Ok(())
}

/// Bulk forget (called by sleep_cycle) — uses existing ACT-R logic
/// but now soft-deletes instead of hard-deleting.
pub fn forget_bulk(&mut self) -> Result<ForgetReport, LifecycleError> {
    // Existing logic: compute activation for all memories
    // Memories below threshold → soft_delete (not hard delete)
    // Hard delete only for memories already soft-deleted for >30 days
    // ...
}
```

**Storage methods**:

```rust
// storage.rs

/// Soft delete: set deleted_at timestamp. (GOAL-15)
pub fn soft_delete(&self, id: &str) -> Result<(), rusqlite::Error> {
    self.conn.execute(
        "UPDATE memories SET deleted_at = ?1 WHERE id = ?2",
        params![Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

/// Hard delete with full cascade. (GOAL-16a)
/// Note: existing `delete_inner()` only handles memories + memories_fts — this fixes
/// orphan problems by cascading to all related tables.
pub fn hard_delete_cascade(&self, id: &str) -> Result<(), rusqlite::Error> {
    // Order matters for foreign key safety:
    self.conn.execute("DELETE FROM memory_embeddings WHERE memory_id = ?1", [id])?;
    self.conn.execute("DELETE FROM access_log WHERE memory_id = ?1", [id])?;
    self.conn.execute("DELETE FROM hebbian_links WHERE source_id = ?1 OR target_id = ?1", [id])?;
    self.conn.execute("DELETE FROM memory_entities WHERE memory_id = ?1", [id])?;
    // Remove from synthesis provenance
    self.conn.execute("DELETE FROM synthesis_provenance WHERE memory_id = ?1", [id])?;
    // Finally remove the memory itself (+ FTS via existing delete_inner pattern)
    self.conn.execute("DELETE FROM memories WHERE id = ?1", [id])?;
    Ok(())
}

/// List soft-deleted memories (GUARD-5).
pub fn list_deleted(&self, namespace: &str) -> Result<Vec<MemoryRecord>, rusqlite::Error> {
    // SELECT * FROM memories WHERE namespace = ? AND deleted_at IS NOT NULL
}
```

**Search exclusion (GUARD-5)**: All search/recall queries add `AND deleted_at IS NULL` to WHERE clause:
- `hybrid_search()` — add filter
- `search_fts()` — add filter  
- `find_nearest_embedding()` — add filter
- `get_all_memories_in_namespace()` — add filter

---

### C7: Sleep Cycle (Enhanced Orchestrator)

**Status**: ~70% done. `sleep_cycle()` runs consolidate → synthesize → forget. Gap: no dedup_scan phase, no decay check, no rebalance, no per-phase timing, not idempotent.

**Design**: Expand sleep_cycle to 5 phases with timing. Decay is folded into the forget phase via `check_decay_and_flag()` (C5) which uses the existing Ebbinghaus model.

```rust
// memory.rs — replace existing sleep_cycle()

pub fn sleep_cycle(&mut self, days: f64, namespace: Option<&str>) -> Result<SleepReport, LifecycleError> {
    let cycle_start = Instant::now();
    let mut report = SleepReport::default();

    // Phase 1: Consolidate (existing — uses `days` for consolidation window)
    let t = Instant::now();
    let consolidation = consolidate_memories(&mut self.storage, &self.config, days, namespace)?;
    report.phases.push(PhaseReport {
        name: "consolidate",
        duration: t.elapsed(),
        count: consolidation.boosted,
    });
    report.consolidated = consolidation.boosted;

    // Phase 2: Dedup scan (NEW — GOAL-19)
    let t = Instant::now();
    let dedup = self.dedup_scan()?;
    report.phases.push(PhaseReport {
        name: "dedup_scan",
        duration: t.elapsed(),
        count: dedup.merged,
    });
    report.dedup_merged = dedup.merged;
    report.dedup_merge_rate = if self.dedup_write_count + self.dedup_merge_count > 0 {
        self.dedup_merge_count as f64 / (self.dedup_write_count + self.dedup_merge_count) as f64
    } else { 0.0 };

    // Phase 3: Synthesis (existing, now incremental — C4)
    let t = Instant::now();
    let synthesis = self.synthesis_engine.run(&mut self.storage, &self.config.synthesis)?;
    report.phases.push(PhaseReport {
        name: "synthesis",
        duration: t.elapsed(),
        count: synthesis.insights_generated,
    });
    report.synthesized = synthesis.insights_generated;
    report.synthesis_incremental = synthesis.synthesis_runs_incremental;

    // Phase 4: Forget (modified — C5 decay check + C6 soft-delete)
    // Decay check uses existing Ebbinghaus model (compute_stability + retrievability)
    // to flag low-strength memories, then bulk forget soft-deletes them.
    let t = Instant::now();
    let decay = self.check_decay_and_flag(namespace)?;
    let forget = self.forget_bulk()?;
    report.phases.push(PhaseReport {
        name: "forget",
        duration: t.elapsed(),
        count: decay.flagged_for_forget + forget.pruned_count,
    });
    report.decay_flagged = decay.flagged_for_forget;
    report.forgotten = forget.pruned_count;

    // Phase 5: Rebalance (NEW — C9)
    let t = Instant::now();
    let rebalance = self.rebalance_internal()?;
    report.phases.push(PhaseReport {
        name: "rebalance",
        duration: t.elapsed(),
        count: rebalance.repairs,
    });
    report.repaired = rebalance.repairs;

    // Reset per-cycle counters
    self.dedup_merge_count = 0;
    self.dedup_write_count = 0;

    report.duration = cycle_start.elapsed();
    Ok(report)
}
```

**Idempotency (GOAL-21)**: Each phase is naturally idempotent:
- consolidate: re-running boosts nothing (timestamps unchanged)
- dedup_scan: already-merged pairs won't match again
- synthesis: incremental check → skips unchanged clusters
- forget (decay+prune): already-deleted memories skip; Ebbinghaus model is deterministic for same inputs
- rebalance: already-repaired items skip

**Enhanced `SleepReport`**:
```rust
pub struct SleepReport {
    // Existing
    pub consolidated: usize,
    pub synthesized: usize,
    pub forgotten: usize,
    pub duration: Duration,
    pub insights_generated: usize,
    // New (GOAL-20)
    pub phases: Vec<PhaseReport>,
    pub dedup_merged: usize,
    pub dedup_merge_rate: f64,
    pub synthesis_incremental: usize,
    pub decay_flagged: usize,  // memories flagged by Ebbinghaus decay check
    pub repaired: usize,
}

pub struct PhaseReport {
    pub name: &'static str,
    pub duration: Duration,
    pub count: usize,
}
```

**`dedup_scan()`** — batch version of dedup for sleep_cycle:
```rust
fn dedup_scan(&mut self) -> Result<DedupScanReport, LifecycleError> {
    // For each namespace, run reconcile-like scan but auto-apply high-confidence merges (≥ 0.90)
    // Lower confidence matches (0.85-0.90) logged but not auto-merged
    let namespaces = self.storage.list_namespaces()?;
    let mut total_merged = 0;
    for ns in &namespaces {
        let candidates = self.reconcile_internal(ns, 0.90)?;  // higher threshold for auto
        for candidate in &candidates {
            self.storage.merge_memory_into(/* ... */)?;
            total_merged += 1;
        }
    }
    Ok(DedupScanReport { merged: total_merged })
}
```

---

### C8: Co-occurrence Auto-create

**Status**: `record_co_occurrence()` exists in storage.rs but is never called automatically. Hebbian links are only created via explicit API.

**Design**: Track recent recalls; auto-create links when two memories are recalled within 30s.

**New field in `Memory` struct**:
```rust
/// Recent recall timestamps for co-occurrence detection.
/// Bounded ring buffer: last 50 recalls.
recent_recalls: VecDeque<(String, Instant)>,  // (memory_id, timestamp)
```

**Co-occurrence injection into existing `recall_from_namespace()`** — ADDITIVE to existing recall logic.
The existing `recall_from_namespace()` has its own inline implementation (embedding similarity + FTS + entity recall + ACT-R boosting). Do NOT replace it. Insert this block at the end, before the return statement. Keep existing return type `Result<Vec<RecallResult>, Box<dyn std::error::Error>>`.

```rust
// At end of existing recall_from_namespace(), before return:
// Co-occurrence detection (GOAL-17) — ADDITIVE to existing recall logic
let now_instant = Instant::now();
let ns = namespace.unwrap_or("default");
for result in &results {
    for (prev_id, prev_time) in &self.recent_recalls {
        if prev_id == &result.record.id { continue; }
        if now_instant.duration_since(*prev_time) <= Duration::from_secs(30) {
            // Within 30s window → record co-activation via existing API
            // record_coactivation_ns signature: (&mut self, id1: &str, id2: &str, threshold: i32, namespace: &str)
            // threshold = coactivation count before forming a Hebbian link (NOT a weight increment)
            let _ = self.storage.record_coactivation_ns(
                prev_id,
                &result.record.id,
                self.config.hebbian_threshold,  // e.g. 3 — coactivation count before link forms
                ns,
            );
        }
    }
    self.recent_recalls.push_back((result.record.id.clone(), now_instant));
    if self.recent_recalls.len() > 50 { self.recent_recalls.pop_front(); }
}
```

**Entity normalization (GOAL-18)** — applied in `extract_entities()`, NOT in recall.
Keep existing `normalize_entity_name` signature which takes `entity_type` for type-specific logic (Person → strip @, URL → strip trailing /, etc.). Enhance the existing function body with whitespace normalization:
```rust
// entities.rs — enhance existing function (signature: entities.rs:272)
pub fn normalize_entity_name(name: &str, entity_type: &EntityType) -> String {
    // Whitespace normalization (GOAL-18 enhancement)
    let mut normalized = name.trim().to_lowercase();
    normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    match entity_type {
        // ... existing type-specific logic preserved (Person → strip @, URL → strip trailing /, etc.)
    }
    normalized
}
```

GOAL-18 normalization is applied in `extract_entities()` before storing in the `memory_entities` table — NOT in recall(). Co-occurrence links are between memory IDs (UUIDs), which don't need normalization.

---

### C9: Rebalance & Health

**Status**: New. No existing code.

**New methods in `memory.rs`**:

```rust
/// Health check report. (GOAL-22)
pub struct HealthReport {
    pub total_memories: usize,
    pub per_namespace: HashMap<String, usize>,
    pub avg_stability: f64,
    pub below_threshold: usize,       // stability < 0.1
    pub orphan_memories: usize,       // no embeddings
    pub stale_clusters: usize,        // clusters with >50% deleted members
    pub dangling_hebbian_links: usize, // links referencing deleted memories
    pub soft_deleted: usize,
}

pub fn health(&self) -> Result<HealthReport, LifecycleError> {
    let total = self.storage.count_memories(None)?;
    let namespaces = self.storage.list_namespaces()?;
    let mut per_ns = HashMap::new();
    for ns in &namespaces {
        per_ns.insert(ns.clone(), self.storage.count_memories(Some(ns))?);
    }

    let avg_stability = self.storage.avg_stability()?;
    let below = self.storage.count_below_stability(0.1)?;
    let orphans = self.storage.count_orphan_memories()?;  // LEFT JOIN embeddings WHERE emb IS NULL
    let stale = self.synthesis_engine.count_stale_clusters(&self.storage)?;
    let dangling = self.storage.count_dangling_hebbian()?;
    let soft_del = self.storage.count_soft_deleted()?;

    Ok(HealthReport {
        total_memories: total,
        per_namespace: per_ns,
        avg_stability,
        below_threshold: below,
        orphan_memories: orphans,
        stale_clusters: stale,
        dangling_hebbian_links: dangling,
        soft_deleted: soft_del,
    })
}

/// Rebalance: repair integrity issues. (GOAL-23)
pub fn rebalance(
    &mut self,
    permission: Permission,
) -> Result<RebalanceReport, LifecycleError> {
    // GUARD-6: admin only
    if !permission.is_admin() {
        return Err(LifecycleError::PermissionDenied {
            op: "rebalance", required: Permission::Admin
        });
    }
    self.rebalance_internal()
}

fn rebalance_internal(&mut self) -> Result<RebalanceReport, LifecycleError> {
    let mut report = RebalanceReport::default();

    // 1. Rebuild missing embeddings
    let orphans = self.storage.get_orphan_memory_ids()?;
    for id in &orphans {
        let content = self.storage.get_memory_content(id)?;
        if let Ok(embedding) = self.embed(&content) {
            self.storage.store_embedding(id, &embedding, &self.config.embedding_model)?;
            report.embeddings_rebuilt += 1;
        }
    }

    // 2. Remove orphaned access_log entries (memory deleted but log remains)
    report.access_log_cleaned = self.storage.cleanup_orphaned_access_log()?;

    // 3. Repair dangling Hebbian links
    report.hebbian_repaired = self.storage.cleanup_dangling_hebbian()?;

    // 4. Cleanup entity_links for deleted memories
    report.entity_links_cleaned = self.storage.cleanup_orphaned_entity_links()?;

    report.repairs = report.embeddings_rebuilt
        + report.access_log_cleaned
        + report.hebbian_repaired
        + report.entity_links_cleaned;

    Ok(report)
}
```

```rust
pub struct RebalanceReport {
    pub embeddings_rebuilt: usize,
    pub access_log_cleaned: usize,
    pub hebbian_repaired: usize,
    pub entity_links_cleaned: usize,
    pub repairs: usize,
}
```

**Storage helpers for health/rebalance**:
```rust
/// Compute average stability across all memories.
/// NOTE: O(N) — loads all records and computes stability from Ebbinghaus model.
/// For large DBs (>10k memories), consider sampling.
pub fn avg_stability(&self) -> Result<f64, rusqlite::Error>;

/// Count memories with stability below threshold.
/// NOTE: O(N) — requires loading all records and computing stability.
pub fn count_below_stability(&self, threshold: f64) -> Result<usize, rusqlite::Error>;
pub fn count_orphan_memories(&self) -> Result<usize, rusqlite::Error>;
pub fn count_dangling_hebbian(&self) -> Result<usize, rusqlite::Error>;
pub fn count_soft_deleted(&self) -> Result<usize, rusqlite::Error>;
pub fn get_orphan_memory_ids(&self) -> Result<Vec<String>, rusqlite::Error>;
pub fn cleanup_orphaned_access_log(&self) -> Result<usize, rusqlite::Error>;
pub fn cleanup_dangling_hebbian(&self) -> Result<usize, rusqlite::Error>;
pub fn cleanup_orphaned_entity_links(&self) -> Result<usize, rusqlite::Error>;
pub fn list_namespaces(&self) -> Result<Vec<String>, rusqlite::Error>;
```

---

## 3. Cross-Cutting Concerns

### Structured Logging (GOAL-24)

All lifecycle operations use `tracing` crate with structured fields:

```rust
tracing::info!(
    operation = "merge",
    memory_id = %target_id,
    donor_id = %donor_id,
    namespace = %namespace,
    similarity = score,
    duration_ms = elapsed.as_millis(),
    "memory merged"
);
```

Standard fields for all lifecycle events:
- `operation`: merge | dedup | decay | forget | rebalance | reconcile | co_occur
- `memory_id`: primary memory involved
- `namespace`: memory namespace
- `duration_ms`: operation time

### DB Migration Plan

Single migration adding soft-delete column (nullable with default, no data rewrite needed):

```sql
-- Migration: feat-003-lifecycle
ALTER TABLE memories ADD COLUMN deleted_at TEXT DEFAULT NULL;

-- Index for soft-delete filter performance
CREATE INDEX IF NOT EXISTS idx_memories_deleted_at ON memories(deleted_at);
```

Migration runs in `Storage::new()` via existing `ensure_schema()` pattern — ALTER TABLE IF NOT EXISTS is safe to re-run.

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("storage: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("permission denied: {op} requires {required:?}")]
    PermissionDenied { op: &'static str, required: Permission },
    #[error("memory not found: {0}")]
    NotFound(String),
    #[error("embedding unavailable for model {0}")]
    EmbeddingUnavailable(String),
    #[error("synthesis: {0}")]
    Synthesis(#[from] SynthesisError),
    #[error("consolidation: {0}")]
    Consolidation(#[from] ConsolidationError),
}
```

### Implementation Order

| Phase | Components | Dependencies | Estimated Lines |
|-------|-----------|-------------|-----------------|
| 1 | C5 (Decay) + C6 (Forget) | DB migration first | ~180 |
| 2 | C1 (Dedup) + C2 (Merge) | None | ~200 |
| 3 | C8 (Co-occur) + C3 (Reconcile) | C1, C2 | ~200 |
| 4 | C4 (Incremental Synthesis) | Existing synthesis engine | ~150 |
| 5 | C7 (Sleep Cycle) + C9 (Rebalance) | All above | ~200 |

**Total: ~930 new/modified lines across 6 files.**

---

## 4. Non-Goals

- **LLM-assisted merge conflict resolution** — out of scope; merge is deterministic
- **Cross-namespace dedup** — only within same namespace
- **Real-time streaming of lifecycle events** — tracing logs only, no event bus
- **Undo for hard delete** — by design; use soft-delete for recoverable operations
