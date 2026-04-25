# Context Assembly

**Tokens**: 2577/12000 | **Nodes**: 120 visited, 120 included, 0 filtered
**Elapsed**: 1ms

## Targets

### `file:src/lifecycle.rs` — lifecycle.rs
**File**: `src/lifecycle.rs`
*~15 tokens*

### `file:src/metacognition.rs` — metacognition.rs
**File**: `src/metacognition.rs`
*~16 tokens*

### `file:src/clustering.rs` — clustering.rs
**File**: `src/clustering.rs`
*~15 tokens*

### `file:src/storage.rs` — storage.rs
**File**: `src/storage.rs`
*~15 tokens*

## Dependencies

- **`module:src`** (`src`) — belongs_to | score: 0.52
## Callers

- **`class:src/lifecycle.rs:AddResult`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `pub enum AddResult`
- **`class:src/lifecycle.rs:DecayReport`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `pub struct DecayReport`
- **`class:src/lifecycle.rs:ForgetReport`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `pub struct ForgetReport`
- **`class:src/lifecycle.rs:HealthReport`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `pub struct HealthReport`
- **`class:src/lifecycle.rs:LifecycleError`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `pub enum LifecycleError`
- **`class:src/lifecycle.rs:PhaseReport`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `pub struct PhaseReport`
- **`class:src/lifecycle.rs:RebalanceReport`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `pub struct RebalanceReport`
- **`class:src/lifecycle.rs:ReconcileCandidate`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `pub struct ReconcileCandidate`
- **`class:src/lifecycle.rs:ReconcileReport`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `pub struct ReconcileReport`
- **`func:src/lifecycle.rs:tests::test_memory`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_memory() -> Memory`
- **`func:src/lifecycle.rs:tests::test_soft_delete_excludes_from_search`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_soft_delete_excludes_from_search()`
- **`func:src/lifecycle.rs:tests::test_hard_delete_cascade`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_hard_delete_cascade()`
- **`func:src/lifecycle.rs:tests::test_forget_targeted_soft`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_forget_targeted_soft()`
- **`func:src/lifecycle.rs:tests::test_forget_targeted_hard`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_forget_targeted_hard()`
- **`func:src/lifecycle.rs:tests::test_count_soft_deleted`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_count_soft_deleted()`
- **`func:src/lifecycle.rs:tests::test_find_entity_overlap`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_find_entity_overlap()`
- **`func:src/lifecycle.rs:tests::test_cross_recall_co_occurrence_tracking`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_cross_recall_co_occurrence_tracking()`
- **`func:src/lifecycle.rs:tests::test_recent_recalls_ring_buffer_cap`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_recent_recalls_ring_buffer_cap()`
- **`func:src/lifecycle.rs:tests::test_reconcile_empty_namespace`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_reconcile_empty_namespace()`
- **`func:src/lifecycle.rs:tests::test_reconcile_apply_dry_run`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_reconcile_apply_dry_run()`
- **`func:src/lifecycle.rs:tests::test_merge_hebbian_links`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_merge_hebbian_links()`
- **`func:src/lifecycle.rs:tests::test_append_merge_provenance`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_append_merge_provenance()`
- **`func:src/lifecycle.rs:tests::test_health_report`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_health_report()`
- **`func:src/lifecycle.rs:tests::test_health_stale_clusters`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_health_stale_clusters()`
- **`func:src/lifecycle.rs:tests::test_rebalance_cleans_orphaned_access_log`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_rebalance_cleans_orphaned_access_log()`
- **`func:src/lifecycle.rs:tests::test_rebalance_cleans_dangling_hebbian`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_rebalance_cleans_dangling_hebbian()`
- **`func:src/lifecycle.rs:tests::test_enhanced_sleep_cycle_phases`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_enhanced_sleep_cycle_phases()`
- **`func:src/lifecycle.rs:tests::test_list_namespaces`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_list_namespaces()`
- **`func:src/lifecycle.rs:tests::test_count_orphan_memories`** (`src/lifecycle.rs`) — defined_in | score: 0.76
  Sig: `fn test_count_orphan_memories()`
- **`infer:component:0.6`** — contains | score: 0.76
  Groups the public API interface and its associated test suites for knowledge compilation.
- **`infer:component:0.15`** — contains | score: 0.76
  Manages entity lifecycle operations with integration testing.
- **`class:src/metacognition.rs:MetaCognitionReport`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `pub struct MetaCognitionReport`
- **`class:src/metacognition.rs:MetaCognitionTracker`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `pub struct MetaCognitionTracker`
- **`class:src/metacognition.rs:ParameterSuggestion`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `pub struct ParameterSuggestion`
- **`class:src/metacognition.rs:RecallEvent`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `pub struct RecallEvent`
- **`class:src/metacognition.rs:SynthesisEvent`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `pub struct SynthesisEvent`
- **`const:src/metacognition.rs:ROLLING_WINDOW_SIZE`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `const ROLLING_WINDOW_SIZE: usize = 500;

// ── Event Types ───────────────────────────────────────────────

/// A recorded recall event with timing and quality signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallEvent`
- **`func:src/metacognition.rs:tests::setup_db`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `fn setup_db() -> Connection`
- **`func:src/metacognition.rs:tests::test_record_and_report_recall`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `fn test_record_and_report_recall()`
- **`func:src/metacognition.rs:tests::test_record_and_report_synthesis`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `fn test_record_and_report_synthesis()`
- **`func:src/metacognition.rs:tests::test_feedback_event`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `fn test_feedback_event()`
- **`func:src/metacognition.rs:tests::test_parameter_suggestions_empty_recalls`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `fn test_parameter_suggestions_empty_recalls()`
- **`func:src/metacognition.rs:tests::test_parameter_suggestions_low_confidence`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `fn test_parameter_suggestions_low_confidence()`
- **`func:src/metacognition.rs:tests::test_rolling_window_cap`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `fn test_rolling_window_cap()`
- **`func:src/metacognition.rs:tests::test_load_history`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `fn test_load_history()`
- **`func:src/metacognition.rs:tests::test_empty_report`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `fn test_empty_report()`
- **`func:src/metacognition.rs:tests::test_suggestions_sorted_by_confidence`** (`src/metacognition.rs`) — defined_in | score: 0.76
  Sig: `fn test_suggestions_sorted_by_confidence()`
- **`infer:component:0.16`** — contains | score: 0.76
  Implements clustering algorithms and metacognitive processing capabilities.
- **`class:src/clustering.rs:ClusteringConfig`** (`src/clustering.rs`) — defined_in | score: 0.76
  Sig: `pub struct ClusteringConfig`
- **`class:src/clustering.rs:Community`** (`src/clustering.rs`) — defined_in | score: 0.76
  Sig: `pub struct Community<T>`
- **`class:src/clustering.rs:CosineStrategy`** (`src/clustering.rs`) — defined_in | score: 0.76
  Sig: `pub struct CosineStrategy`
- **`class:src/clustering.rs:EdgeWeightStrategy`** (`src/clustering.rs`) — defined_in | score: 0.76
  Sig: `pub trait EdgeWeightStrategy`
- **`class:src/clustering.rs:EmbeddingItem`** (`src/clustering.rs`) — defined_in | score: 0.76
  Sig: `pub struct EmbeddingItem`
- **`func:src/clustering.rs:cluster_with_infomap`** (`src/clustering.rs`) — defined_in | score: 0.76
  Sig: `pub fn cluster_with_infomap<T, S>(
    items: &[T],
    strategy: &S,
    config: &ClusteringConfig,
) -> Vec<Community<T>>
where
    T: Clone,
    S: EdgeWeightStrategy<Item = T>,
{
    let n = items.len();
    if n < 2`
- **`func:src/clustering.rs:tests::test_cosine_strategy_basic`** (`src/clustering.rs`) — defined_in | score: 0.76
  Sig: `fn test_cosine_strategy_basic()`
- **`func:src/clustering.rs:tests::test_empty_input`** (`src/clustering.rs`) — defined_in | score: 0.76
  Sig: `fn test_empty_input()`
- **`func:src/clustering.rs:tests::test_no_edges_above_threshold`** (`src/clustering.rs`) — defined_in | score: 0.76
  Sig: `fn test_no_edges_above_threshold()`
- **`func:src/clustering.rs:tests::test_custom_strategy`** (`src/clustering.rs`) — defined_in | score: 0.76
  Sig: `fn test_custom_strategy()`
- **`func:src/clustering.rs:tests::test_min_community_size_filter`** (`src/clustering.rs`) — defined_in | score: 0.76
  Sig: `fn test_min_community_size_filter()`
- **`class:src/storage.rs:BackfillRow`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `pub struct BackfillRow`
- **`class:src/storage.rs:EmbeddingStats`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `pub struct EmbeddingStats`
- **`class:src/storage.rs:EntityRecord`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `pub struct EntityRecord`
- **`class:src/storage.rs:QuarantineRow`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `pub struct QuarantineRow`
- **`class:src/storage.rs:Storage`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `pub struct Storage`
- **`func:src/storage.rs:jieba`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn jieba() -> &'static jieba_rs::Jieba`
- **`func:src/storage.rs:tokenize_cjk_boundaries`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn tokenize_cjk_boundaries(text: &str) -> String`
- **`func:src/storage.rs:tokenize_like_unicode61`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn tokenize_like_unicode61(text: &str) -> Vec<String>`
- **`func:src/storage.rs:is_cjk_char`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn is_cjk_char(ch: char) -> bool`
- **`func:src/storage.rs:datetime_to_f64`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn datetime_to_f64(dt: &DateTime<Utc>) -> f64`
- **`func:src/storage.rs:f64_to_datetime`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn f64_to_datetime(ts: f64) -> DateTime<Utc>`
- **`func:src/storage.rs:now_f64`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `pub fn now_f64() -> f64`
- **`func:src/storage.rs:bytes_to_f32_vec`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32>`
- **`func:src/storage.rs:generate_entity_id`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn generate_entity_id(name: &str, entity_type: &str, namespace: &str) -> String`
- **`func:src/storage.rs:read_merge_tracking`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn read_merge_tracking(
    metadata: &serde_json::Value,
) -> (Vec<serde_json::Value>, i64)`
- **`func:src/storage.rs:write_merge_tracking`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn write_merge_tracking(
    metadata: &mut serde_json::Value,
    history: Vec<serde_json::Value>,
    count: i64,
)`
- **`func:src/storage.rs:tests::test_storage`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_storage() -> Storage`
- **`func:src/storage.rs:tests::make_record`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn make_record(id: &str, content: &str, created_at: DateTime<Utc>) -> MemoryRecord`
- **`func:src/storage.rs:tests::test_record_association_new`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_record_association_new()`
- **`func:src/storage.rs:tests::test_record_association_duplicate`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_record_association_duplicate()`
- **`func:src/storage.rs:tests::test_record_association_bidirectional`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_record_association_bidirectional()`
- **`func:src/storage.rs:tests::test_decay_differential_rates`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_decay_differential_rates()`
- **`func:src/storage.rs:tests::test_decay_differential_deletes_weak`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_decay_differential_deletes_weak()`
- **`func:src/storage.rs:tests::test_hebbian_signal_migration_fresh_db`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_hebbian_signal_migration_fresh_db()`
- **`func:src/storage.rs:tests::test_hebbian_signal_migration_idempotent`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_hebbian_signal_migration_idempotent()`
- **`func:src/storage.rs:tests::test_hebbian_signal_migration_backfills_existing_rows`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_hebbian_signal_migration_backfills_existing_rows()`
- **`func:src/storage.rs:tests::test_cluster_centroids_roundtrip`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_cluster_centroids_roundtrip()`
- **`func:src/storage.rs:tests::test_assign_to_cluster`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_assign_to_cluster()`
- **`func:src/storage.rs:tests::test_centroid_incremental_update`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_centroid_incremental_update()`
- **`func:src/storage.rs:tests::test_dirty_cluster_tracking`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_dirty_cluster_tracking()`
- **`func:src/storage.rs:tests::test_pending_memory_tracking`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_pending_memory_tracking()`
- **`func:src/storage.rs:tests::test_replace_clusters`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_replace_clusters()`
- **`func:src/storage.rs:tests::test_save_full_cluster_state`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_save_full_cluster_state()`
- **`func:src/storage.rs:tests::test_get_memories_by_ids_empty`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_get_memories_by_ids_empty()`
- **`func:src/storage.rs:tests::make_enriched`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn make_enriched(content: &str, importance: f64) -> crate::enriched::EnrichedMemory`
- **`func:src/storage.rs:tests::persist_enriched`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn persist_enriched(
        storage: &mut Storage,
        id: &str,
        em: &crate::enriched::EnrichedMemory,
    ) -> String`
- **`func:src/storage.rs:tests::test_merge_enriched_into_applies_union`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_merge_enriched_into_applies_union()`
- **`func:src/storage.rs:tests::test_merge_enriched_into_increments_merge_count`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_merge_enriched_into_increments_merge_count()`
- **`func:src/storage.rs:tests::test_merge_enriched_into_history_fifo_capped_at_10`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_merge_enriched_into_history_fifo_capped_at_10()`
- **`func:src/storage.rs:tests::test_merge_enriched_into_idempotent_on_identical_inputs`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_merge_enriched_into_idempotent_on_identical_inputs()`
- **`func:src/storage.rs:tests::test_merge_enriched_into_longer_content_wins`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_merge_enriched_into_longer_content_wins()`
- **`func:src/storage.rs:tests::test_merge_enriched_into_missing_id_errors`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_merge_enriched_into_missing_id_errors()`
- **`func:src/storage.rs:tests::test_quarantine_insert_and_list`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_quarantine_insert_and_list()`
- **`func:src/storage.rs:tests::test_quarantine_insert_dedups_on_live_hash`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_quarantine_insert_dedups_on_live_hash()`
- **`func:src/storage.rs:tests::test_quarantine_insert_skips_dedup_for_rejected_rows`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_quarantine_insert_skips_dedup_for_rejected_rows()`
- **`func:src/storage.rs:tests::test_quarantine_record_attempt_and_mark_rejected`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_quarantine_record_attempt_and_mark_rejected()`
- **`func:src/storage.rs:tests::test_quarantine_delete_row`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_quarantine_delete_row()`
- **`func:src/storage.rs:tests::test_quarantine_list_batch_limit_and_ordering`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_quarantine_list_batch_limit_and_ordering()`
- **`func:src/storage.rs:tests::test_quarantine_purge_respects_ttl_and_flag`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_quarantine_purge_respects_ttl_and_flag()`
- **`func:src/storage.rs:tests::test_backfill_enqueue_and_list`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_backfill_enqueue_and_list()`
- **`func:src/storage.rs:tests::test_backfill_enqueue_is_idempotent_on_live_row`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_backfill_enqueue_is_idempotent_on_live_row()`
- **`func:src/storage.rs:tests::test_backfill_enqueue_skips_rejected_row_update`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_backfill_enqueue_skips_rejected_row_update()`
- **`func:src/storage.rs:tests::test_backfill_record_attempt_and_reject`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_backfill_record_attempt_and_reject()`
- **`func:src/storage.rs:tests::test_backfill_delete_row`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_backfill_delete_row()`
- **`func:src/storage.rs:tests::test_backfill_list_batch_limit_and_ordering`** (`src/storage.rs`) — defined_in | score: 0.76
  Sig: `fn test_backfill_list_batch_limit_and_ordering()`
- **`infer:component:infrastructure`** — contains | score: 0.76
  Core platform infrastructure including compiler, storage, discovery, embeddings, and event bus systems.

context: 120 visited, 120 included, 0 filtered, 2577/12000 tokens, 1ms
