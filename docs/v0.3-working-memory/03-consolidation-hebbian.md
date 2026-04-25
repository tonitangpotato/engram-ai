# Context Assembly

**Tokens**: 3524/15000 | **Nodes**: 144 visited, 144 included, 0 filtered
**Elapsed**: 1ms

## Targets

### `file:src/models/hebbian.rs` — hebbian.rs
**File**: `src/models/hebbian.rs`
*~15 tokens*

### `file:src/models/consolidation.rs` — consolidation.rs
**File**: `src/models/consolidation.rs`
*~16 tokens*

### `file:src/models/actr.rs` — actr.rs
**File**: `src/models/actr.rs`
*~14 tokens*

### `file:src/models/ebbinghaus.rs` — ebbinghaus.rs
**File**: `src/models/ebbinghaus.rs`
*~15 tokens*

### `file:src/association/former.rs` — former.rs
**File**: `src/association/former.rs`
*~14 tokens*

### `file:src/association/signals.rs` — signals.rs
**File**: `src/association/signals.rs`
*~15 tokens*

### `file:src/promotion.rs` — promotion.rs
**File**: `src/promotion.rs`
*~15 tokens*

### `file:src/memory.rs` — memory.rs
**File**: `src/memory.rs`
*~14 tokens*

## Dependencies

- **`module:src/models`** (`src/models`) — belongs_to | score: 0.52
- **`module:src/association`** (`src/association`) — belongs_to | score: 0.52
- **`module:src`** (`src`) — belongs_to | score: 0.52
## Callers

- **`class:src/models/hebbian.rs:MemoryWithNamespace`** (`src/models/hebbian.rs`) — defined_in | score: 0.76
  Sig: `pub struct MemoryWithNamespace`
- **`func:src/models/hebbian.rs:record_coactivation`** (`src/models/hebbian.rs`) — defined_in | score: 0.76
  Sig: `pub fn record_coactivation(
    storage: &mut Storage,
    memory_ids: &[String],
    threshold: i32,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>>`
- **`func:src/models/hebbian.rs:record_coactivation_ns`** (`src/models/hebbian.rs`) — defined_in | score: 0.76
  Sig: `pub fn record_coactivation_ns(
    storage: &mut Storage,
    memory_ids: &[String],
    threshold: i32,
    namespace: &str,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>>`
- **`func:src/models/hebbian.rs:record_cross_namespace_coactivation`** (`src/models/hebbian.rs`) — defined_in | score: 0.76
  Sig: `pub fn record_cross_namespace_coactivation(
    storage: &mut Storage,
    memories: &[MemoryWithNamespace],
    threshold: i32,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>>`
- **`func:src/models/hebbian.rs:discover_cross_links`** (`src/models/hebbian.rs`) — defined_in | score: 0.76
  Sig: `pub fn discover_cross_links(
    storage: &Storage,
    namespace_a: &str,
    namespace_b: &str,
) -> Result<Vec<HebbianLink>, Box<dyn std::error::Error>>`
- **`func:src/models/hebbian.rs:get_cross_namespace_associations`** (`src/models/hebbian.rs`) — defined_in | score: 0.76
  Sig: `pub fn get_cross_namespace_associations(
    storage: &Storage,
    memory_id: &str,
) -> Result<Vec<CrossLink>, Box<dyn std::error::Error>>`
- **`infer:component:0.5`** — contains | score: 0.76
  Accumulator and feedback mechanisms for the message bus subsystem.
- **`infer:component:infrastructure`** — contains | score: 0.76
  Core platform infrastructure including compiler, storage, discovery, embeddings, and event bus systems.
- **`func:src/models/consolidation.rs:apply_decay`** (`src/models/consolidation.rs`) — defined_in | score: 0.76
  Sig: `pub fn apply_decay(record: &mut MemoryRecord, dt_days: f64, mu1: f64, mu2: f64)`
- **`func:src/models/consolidation.rs:consolidate_single`** (`src/models/consolidation.rs`) — defined_in | score: 0.76
  Sig: `pub fn consolidate_single(record: &mut MemoryRecord, dt_days: f64, config: &MemoryConfig)`
- **`func:src/models/consolidation.rs:run_consolidation_cycle`** (`src/models/consolidation.rs`) — defined_in | score: 0.76
  Sig: `pub fn run_consolidation_cycle(
    storage: &mut Storage,
    dt_days: f64,
    config: &MemoryConfig,
    namespace: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>>`
- **`func:src/models/consolidation.rs:rebalance_layers`** (`src/models/consolidation.rs`) — defined_in | score: 0.76
  Sig: `fn rebalance_layers(memories: &mut [MemoryRecord], config: &MemoryConfig)`
- **`infer:component:0.18`** — contains | score: 0.76
  Implements the consolidation model for memory processing.
- **`func:src/models/actr.rs:base_level_activation`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `pub fn base_level_activation(record: &MemoryRecord, now: DateTime<Utc>, decay: f64) -> f64`
- **`func:src/models/actr.rs:spreading_activation`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `pub fn spreading_activation(record: &MemoryRecord, context_keywords: &[String], weight: f64) -> f64`
- **`func:src/models/actr.rs:retrieval_activation`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `pub fn retrieval_activation(
    record: &MemoryRecord,
    context_keywords: &[String],
    now: DateTime<Utc>,
    base_decay: f64,
    context_weight: f64,
    importance_weight: f64,
    contradiction_penalty: f64,
) -> f64`
- **`func:src/models/actr.rs:normalize_activation`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `pub fn normalize_activation(activation: f64, center: f64, scale: f64) -> f64`
- **`func:src/models/actr.rs:tests::make_record`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn make_record(age_secs: i64) -> (MemoryRecord, DateTime<Utc>)`
- **`func:src/models/actr.rs:tests::test_normalize_neg_infinity`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn test_normalize_neg_infinity()`
- **`func:src/models/actr.rs:tests::test_normalize_center_gives_half`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn test_normalize_center_gives_half()`
- **`func:src/models/actr.rs:tests::test_normalize_monotonic`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn test_normalize_monotonic()`
- **`func:src/models/actr.rs:tests::test_normalize_bounded`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn test_normalize_bounded()`
- **`func:src/models/actr.rs:tests::test_recency_discrimination_improved`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn test_recency_discrimination_improved()`
- **`func:src/models/actr.rs:tests::test_base_level_recency`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn test_base_level_recency()`
- **`func:src/models/actr.rs:tests::test_base_level_frequency`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn test_base_level_frequency()`
- **`func:src/models/actr.rs:tests::test_base_level_no_accesses`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn test_base_level_no_accesses()`
- **`func:src/models/actr.rs:tests::test_spreading_activation_matches`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn test_spreading_activation_matches()`
- **`func:src/models/actr.rs:tests::test_spreading_activation_no_match`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn test_spreading_activation_no_match()`
- **`func:src/models/actr.rs:tests::test_normalize_scale_effect`** (`src/models/actr.rs`) — defined_in | score: 0.76
  Sig: `fn test_normalize_scale_effect()`
- **`infer:component:0.19`** — contains | score: 0.76
  Implements cognitive memory models including ACT-R and Ebbinghaus forgetting curve.
- **`func:src/models/ebbinghaus.rs:retrievability`** (`src/models/ebbinghaus.rs`) — defined_in | score: 0.76
  Sig: `pub fn retrievability(record: &MemoryRecord, now: DateTime<Utc>) -> f64`
- **`func:src/models/ebbinghaus.rs:compute_stability`** (`src/models/ebbinghaus.rs`) — defined_in | score: 0.76
  Sig: `pub fn compute_stability(record: &MemoryRecord) -> f64`
- **`func:src/models/ebbinghaus.rs:effective_strength`** (`src/models/ebbinghaus.rs`) — defined_in | score: 0.76
  Sig: `pub fn effective_strength(record: &MemoryRecord, now: DateTime<Utc>) -> f64`
- **`func:src/models/ebbinghaus.rs:should_forget`** (`src/models/ebbinghaus.rs`) — defined_in | score: 0.76
  Sig: `pub fn should_forget(record: &MemoryRecord, threshold: f64, now: DateTime<Utc>) -> bool`
- **`class:src/association/former.rs:LinkFormer`** (`src/association/former.rs`) — defined_in | score: 0.76
  Sig: `pub struct LinkFormer<'a>`
- **`class:src/association/former.rs:ProtoLink`** (`src/association/former.rs`) — defined_in | score: 0.76
  Sig: `struct ProtoLink`
- **`func:src/association/former.rs:tests::test_storage`** (`src/association/former.rs`) — defined_in | score: 0.76
  Sig: `fn test_storage() -> Storage`
- **`func:src/association/former.rs:tests::make_record`** (`src/association/former.rs`) — defined_in | score: 0.76
  Sig: `fn make_record(id: &str, content: &str, created_at: chrono::DateTime<Utc>) -> MemoryRecord`
- **`func:src/association/former.rs:tests::store_memory_with_entities`** (`src/association/former.rs`) — defined_in | score: 0.76
  Sig: `fn store_memory_with_entities(
        storage: &mut Storage,
        id: &str,
        content: &str,
        entities: &[&str],
        timestamp: chrono::DateTime<Utc>,
    )`
- **`func:src/association/former.rs:tests::store_memory_with_embedding`** (`src/association/former.rs`) — defined_in | score: 0.76
  Sig: `fn store_memory_with_embedding(
        storage: &mut Storage,
        id: &str,
        content: &str,
        embedding: &[f32],
        timestamp: chrono::DateTime<Utc>,
    )`
- **`func:src/association/former.rs:tests::test_discover_no_candidates`** (`src/association/former.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_no_candidates()`
- **`func:src/association/former.rs:tests::test_discover_below_threshold`** (`src/association/former.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_below_threshold()`
- **`func:src/association/former.rs:tests::test_discover_creates_links`** (`src/association/former.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_creates_links()`
- **`func:src/association/former.rs:tests::test_discover_respects_max_links`** (`src/association/former.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_respects_max_links()`
- **`func:src/association/former.rs:tests::test_discover_link_metadata`** (`src/association/former.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_link_metadata()`
- **`infer:component:0.10`** — contains | score: 0.76
  Integrates large language model functionality with file system watching capabilities.
- **`infer:component:0.1.0`** — contains | score: 0.76
  Core association logic including candidate selection, former management, topic lifecycle, and provenance tracking.
- **`class:src/association/signals.rs:SignalComputer`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `pub struct SignalComputer;

impl SignalComputer`
- **`class:src/association/signals.rs:SignalScores`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `pub struct SignalScores`
- **`func:src/association/signals.rs:tests::test_entity_jaccard_no_overlap`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_entity_jaccard_no_overlap()`
- **`func:src/association/signals.rs:tests::test_entity_jaccard_full_overlap`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_entity_jaccard_full_overlap()`
- **`func:src/association/signals.rs:tests::test_entity_jaccard_partial`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_entity_jaccard_partial()`
- **`func:src/association/signals.rs:tests::test_entity_jaccard_both_empty`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_entity_jaccard_both_empty()`
- **`func:src/association/signals.rs:tests::test_embedding_cosine_identical`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_embedding_cosine_identical()`
- **`func:src/association/signals.rs:tests::test_embedding_cosine_orthogonal`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_embedding_cosine_orthogonal()`
- **`func:src/association/signals.rs:tests::test_embedding_cosine_none`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_embedding_cosine_none()`
- **`func:src/association/signals.rs:tests::test_temporal_same_time`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_temporal_same_time()`
- **`func:src/association/signals.rs:tests::test_temporal_distant`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_temporal_distant()`
- **`func:src/association/signals.rs:tests::test_signal_source_multi`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_signal_source_multi()`
- **`func:src/association/signals.rs:tests::test_signal_source_single`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_signal_source_single()`
- **`func:src/association/signals.rs:tests::test_signal_source_single_embedding`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_signal_source_single_embedding()`
- **`func:src/association/signals.rs:tests::test_signal_source_single_temporal`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_signal_source_single_temporal()`
- **`func:src/association/signals.rs:tests::test_combined_score`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_combined_score()`
- **`func:src/association/signals.rs:tests::test_compute_all`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_compute_all()`
- **`func:src/association/signals.rs:tests::test_to_json`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_to_json()`
- **`func:src/association/signals.rs:tests::test_dominant_signal_entity`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_dominant_signal_entity()`
- **`func:src/association/signals.rs:tests::test_dominant_signal_embedding`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_dominant_signal_embedding()`
- **`func:src/association/signals.rs:tests::test_dominant_signal_temporal`** (`src/association/signals.rs`) — defined_in | score: 0.76
  Sig: `fn test_dominant_signal_temporal()`
- **`infer:component:0.1.1`** — contains | score: 0.76
  Signal processing and handling for the association subsystem.
- **`class:src/promotion.rs:PromotionCandidate`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `pub struct PromotionCandidate`
- **`func:src/promotion.rs:default_status`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn default_status() -> String`
- **`func:src/promotion.rs:suggest_target`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn suggest_target(snippets: &[String]) -> String`
- **`func:src/promotion.rs:candidate_id`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn candidate_id(member_ids: &[String]) -> String`
- **`func:src/promotion.rs:connected_components`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn connected_components(adj: &HashMap<String, HashSet<String>>, nodes: &HashSet<String>) -> Vec<Vec<String>>`
- **`func:src/promotion.rs:detect_promotable_clusters`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `pub fn detect_promotable_clusters(
    storage: &Storage,
    config: &PromotionConfig,
) -> Result<Vec<PromotionCandidate>, Box<dyn std::error::Error>>`
- **`func:src/promotion.rs:tests::make_record`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn make_record(id: &str, content: &str, core_strength: f64, importance: f64, days_ago: i64) -> MemoryRecord`
- **`func:src/promotion.rs:tests::add_hebbian_link`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn add_hebbian_link(storage: &Storage, src: &str, tgt: &str, strength: f64)`
- **`func:src/promotion.rs:tests::test_detect_empty`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn test_detect_empty()`
- **`func:src/promotion.rs:tests::test_detect_cluster`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn test_detect_cluster()`
- **`func:src/promotion.rs:tests::test_dedup_already_promoted`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn test_dedup_already_promoted()`
- **`func:src/promotion.rs:tests::test_suggest_target_soul`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn test_suggest_target_soul()`
- **`func:src/promotion.rs:tests::test_suggest_target_agents`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn test_suggest_target_agents()`
- **`func:src/promotion.rs:tests::test_suggest_target_default`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn test_suggest_target_default()`
- **`func:src/promotion.rs:tests::test_candidate_id_deterministic`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn test_candidate_id_deterministic()`
- **`func:src/promotion.rs:tests::test_pending_promotions`** (`src/promotion.rs`) — defined_in | score: 0.76
  Sig: `fn test_pending_promotions()`
- **`infer:component:0.6`** — contains | score: 0.76
  Groups the public API interface and its associated test suites for knowledge compilation.
- **`infer:component:0.1.2`** — contains | score: 0.76
  Handles promotion of candidates or elements within the system.
- **`class:src/memory.rs:Memory`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `pub struct Memory`
- **`class:src/memory.rs:SleepReport`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `pub struct SleepReport`
- **`func:src/memory.rs:is_insight`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `pub fn is_insight(record: &MemoryRecord) -> bool`
- **`func:src/memory.rs:default_event_sink_placeholder`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn default_event_sink_placeholder() -> crate::write_stats::SharedSink`
- **`func:src/memory.rs:compute_query_confidence`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn compute_query_confidence(
    embedding_similarity: Option<f32>,  // None if no embedding available
    in_fts_results: bool,
    entity_score: f64,  // 0.0 if no entity match or entity disabled
    age_hours: f64,
) -> f64`
- **`func:src/memory.rs:confidence_label`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn confidence_label(confidence: f64) -> String`
- **`func:src/memory.rs:detect_feedback_polarity`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn detect_feedback_polarity(feedback: &str) -> f64`
- **`func:src/memory.rs:build_legacy_metadata`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn build_legacy_metadata(mem: &crate::enriched::EnrichedMemory) -> serde_json::Value`
- **`func:src/memory.rs:domain_to_loose_str`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn domain_to_loose_str(d: &crate::dimensions::Domain) -> String`
- **`func:src/memory.rs:boxed_err_to_store_error`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn boxed_err_to_store_error(
    e: Box<dyn std::error::Error>,
) -> crate::store_api::StoreError`
- **`func:src/memory.rs:short_hash`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn short_hash(content: &str) -> String`
- **`func:src/memory.rs:type_weights_favoring`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn type_weights_favoring(mt: crate::types::MemoryType) -> crate::type_weights::TypeWeights`
- **`func:src/memory.rs:confidence_tests::test_confidence_high_embedding_sim`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_high_embedding_sim()`
- **`func:src/memory.rs:confidence_tests::test_confidence_low_embedding_sim`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_low_embedding_sim()`
- **`func:src/memory.rs:confidence_tests::test_confidence_medium_embedding_sim`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_medium_embedding_sim()`
- **`func:src/memory.rs:confidence_tests::test_confidence_no_embedding`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_no_embedding()`
- **`func:src/memory.rs:confidence_tests::test_confidence_fts_only_boost`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_fts_only_boost()`
- **`func:src/memory.rs:confidence_tests::test_confidence_entity_boost`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_entity_boost()`
- **`func:src/memory.rs:confidence_tests::test_confidence_recency_boost`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_recency_boost()`
- **`func:src/memory.rs:confidence_tests::test_confidence_all_zero`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_all_zero()`
- **`func:src/memory.rs:confidence_tests::test_confidence_all_max`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_all_max()`
- **`func:src/memory.rs:confidence_tests::test_confidence_label_thresholds`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_label_thresholds()`
- **`func:src/memory.rs:confidence_tests::test_confidence_sigmoid_discrimination`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_sigmoid_discrimination()`
- **`func:src/memory.rs:confidence_tests::test_auto_extract_importance_cap`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_auto_extract_importance_cap()`
- **`func:src/memory.rs:confidence_tests::test_auto_extract_importance_cap_default`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_auto_extract_importance_cap_default()`
- **`func:src/memory.rs:confidence_tests::test_dedup_recall_results_by_embedding`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_dedup_recall_results_by_embedding()`
- **`func:src/memory.rs:confidence_tests::test_dedup_recall_no_duplicates`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_dedup_recall_no_duplicates()`
- **`func:src/memory.rs:confidence_tests::test_dedup_recall_respects_limit`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_dedup_recall_respects_limit()`
- **`func:src/memory.rs:confidence_tests::test_dedup_recall_missing_embedding_kept`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_dedup_recall_missing_embedding_kept()`
- **`func:src/memory.rs:confidence_tests::test_dedup_recall_backfills_from_candidates`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_dedup_recall_backfills_from_candidates()`
- **`func:src/memory.rs:confidence_tests::test_recall_dedup_config_defaults`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_recall_dedup_config_defaults()`
- **`func:src/memory.rs:confidence_tests::make_test_record`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn make_test_record(id: &str, content: &str, created_at: chrono::DateTime<Utc>) -> MemoryRecord`
- **`func:src/memory.rs:confidence_tests::test_temporal_score_within_range`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_temporal_score_within_range()`
- **`func:src/memory.rs:confidence_tests::test_temporal_score_outside_range`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_temporal_score_outside_range()`
- **`func:src/memory.rs:confidence_tests::test_temporal_score_no_range`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_temporal_score_no_range()`
- **`func:src/memory.rs:confidence_tests::test_hebbian_channel_scores_basic`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_hebbian_channel_scores_basic()`
- **`func:src/memory.rs:confidence_tests::test_c7_config_defaults`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_c7_config_defaults()`
- **`func:src/memory.rs:confidence_tests::test_adaptive_weights_disabled_preserves_behavior`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_adaptive_weights_disabled_preserves_behavior()`
- **`func:src/memory.rs:confidence_tests::test_broadcast_admission_generates_confidence_signals`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_broadcast_admission_generates_confidence_signals()`
- **`func:src/memory.rs:confidence_tests::test_broadcast_admission_multiple_memories`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_broadcast_admission_multiple_memories()`
- **`func:src/memory.rs:confidence_tests::test_broadcast_with_nonexistent_memory_is_safe`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_broadcast_with_nonexistent_memory_is_safe()`
- **`func:src/memory.rs:confidence_tests::test_broadcast_updates_hub_domain_state`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_broadcast_updates_hub_domain_state()`
- **`func:src/memory.rs:confidence_tests::test_broadcast_hebbian_spreading`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_broadcast_hebbian_spreading()`
- **`func:src/memory.rs:confidence_tests::test_iss020_p0_0_dimensions_persist_valence_confidence_domain`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_iss020_p0_0_dimensions_persist_valence_confidence_domain()`
- **`func:src/memory.rs:confidence_tests::test_iss020_p0_0_neutral_valence_still_persisted`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn test_iss020_p0_0_neutral_valence_still_persisted()`
- **`func:src/memory.rs:jaccard_similarity_strings`** (`src/memory.rs`) — defined_in | score: 0.76
  Sig: `fn jaccard_similarity_strings(a: &[String], b: &[String]) -> f64`

context: 144 visited, 144 included, 0 filtered, 3524/15000 tokens, 1ms
