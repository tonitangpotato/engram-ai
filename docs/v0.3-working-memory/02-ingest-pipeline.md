# Context Assembly

**Tokens**: 3600/15000 | **Nodes**: 150 visited, 150 included, 0 filtered
**Elapsed**: 1ms

## Targets

### `file:src/triple.rs` — triple.rs
**File**: `src/triple.rs`
*~14 tokens*

### `file:src/triple_extractor.rs` — triple_extractor.rs
**File**: `src/triple_extractor.rs`
*~17 tokens*

### `file:src/extractor.rs` — extractor.rs
**File**: `src/extractor.rs`
*~15 tokens*

### `file:src/enriched.rs` — enriched.rs
**File**: `src/enriched.rs`
*~15 tokens*

### `file:src/bus/mod.rs` — mod.rs
**File**: `src/bus/mod.rs`
*~14 tokens*

### `file:src/bus/accumulator.rs` — accumulator.rs
**File**: `src/bus/accumulator.rs`
*~16 tokens*

### `file:src/entities.rs` — entities.rs
**File**: `src/entities.rs`
*~15 tokens*

## Dependencies

- **`module:src`** (`src`) — belongs_to | score: 0.52
- **`module:src/bus`** (`src/bus`) — belongs_to | score: 0.52
## Callers

- **`class:src/triple.rs:Predicate`** (`src/triple.rs`) — defined_in | score: 0.76
  Sig: `pub enum Predicate`
- **`class:src/triple.rs:Triple`** (`src/triple.rs`) — defined_in | score: 0.76
  Sig: `pub struct Triple`
- **`class:src/triple.rs:TripleSource`** (`src/triple.rs`) — defined_in | score: 0.76
  Sig: `pub enum TripleSource`
- **`func:src/triple.rs:tests::test_predicate_round_trip`** (`src/triple.rs`) — defined_in | score: 0.76
  Sig: `fn test_predicate_round_trip()`
- **`func:src/triple.rs:tests::test_unknown_predicate_falls_back_to_related_to`** (`src/triple.rs`) — defined_in | score: 0.76
  Sig: `fn test_unknown_predicate_falls_back_to_related_to()`
- **`func:src/triple.rs:tests::test_triple_new_clamps_confidence`** (`src/triple.rs`) — defined_in | score: 0.76
  Sig: `fn test_triple_new_clamps_confidence()`
- **`func:src/triple.rs:tests::test_triple_source_serde`** (`src/triple.rs`) — defined_in | score: 0.76
  Sig: `fn test_triple_source_serde()`
- **`func:src/triple.rs:tests::test_predicate_display`** (`src/triple.rs`) — defined_in | score: 0.76
  Sig: `fn test_predicate_display()`
- **`infer:component:0.6`** — contains | score: 0.76
  Groups the public API interface and its associated test suites for knowledge compilation.
- **`infer:component:infrastructure`** — contains | score: 0.76
  Core platform infrastructure including compiler, storage, discovery, embeddings, and event bus systems.
- **`class:src/triple_extractor.rs:AnthropicTripleExtractor`** (`src/triple_extractor.rs`) — defined_in | score: 0.76
  Sig: `pub struct AnthropicTripleExtractor`
- **`class:src/triple_extractor.rs:OllamaTripleExtractor`** (`src/triple_extractor.rs`) — defined_in | score: 0.76
  Sig: `pub struct OllamaTripleExtractor`
- **`class:src/triple_extractor.rs:TripleExtractor`** (`src/triple_extractor.rs`) — defined_in | score: 0.76
  Sig: `pub trait TripleExtractor: Send + Sync`
- **`const:src/triple_extractor.rs:TRIPLE_EXTRACTION_PROMPT`** (`src/triple_extractor.rs`) — defined_in | score: 0.76
  Sig: `const TRIPLE_EXTRACTION_PROMPT: &str = r#"Extract subject-predicate-object triples from the following text.

Allowed predicates: is_a, part_of, uses, depends_on, caused_by, leads_to, implements, contradicts, related_to

Return ONLY a JSON array (no markdown, no explanation):
[{"subject": "...", "predicate": "...", "object": "...", "confidence": 0.X}]

Examples:
Input: "Rust's borrow checker prevents data races at compile time"
Output: [{"subject": "borrow checker", "predicate": "part_of", "object": "Rust", "confidence": 0.9},`
- **`func:src/triple_extractor.rs:parse_triple_response`** (`src/triple_extractor.rs`) — defined_in | score: 0.76
  Sig: `fn parse_triple_response(content: &str) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>>`
- **`func:src/triple_extractor.rs:tests::test_parse_triple_response_clean`** (`src/triple_extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_triple_response_clean()`
- **`func:src/triple_extractor.rs:tests::test_parse_triple_response_markdown`** (`src/triple_extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_triple_response_markdown()`
- **`func:src/triple_extractor.rs:tests::test_parse_triple_response_empty`** (`src/triple_extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_triple_response_empty()`
- **`func:src/triple_extractor.rs:tests::test_parse_triple_response_invalid`** (`src/triple_extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_triple_response_invalid()`
- **`func:src/triple_extractor.rs:tests::test_parse_triple_response_unknown_predicate`** (`src/triple_extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_triple_response_unknown_predicate()`
- **`func:src/triple_extractor.rs:tests::test_parse_triple_response_clamps_confidence`** (`src/triple_extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_triple_response_clamps_confidence()`
- **`infer:component:0.28.1`** — contains | score: 0.76
  Knowledge graph triple extraction from text with integration tests.
- **`class:src/extractor.rs:AnthropicExtractor`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `pub struct AnthropicExtractor`
- **`class:src/extractor.rs:AnthropicExtractorConfig`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `pub struct AnthropicExtractorConfig`
- **`class:src/extractor.rs:DimensionalResponse`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `struct DimensionalResponse`
- **`class:src/extractor.rs:ExtractedFact`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `pub struct ExtractedFact`
- **`class:src/extractor.rs:LegacyExtractedFact`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `struct LegacyExtractedFact`
- **`class:src/extractor.rs:MemoryExtractor`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `pub trait MemoryExtractor: Send + Sync`
- **`class:src/extractor.rs:OllamaExtractor`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `pub struct OllamaExtractor`
- **`class:src/extractor.rs:OllamaExtractorConfig`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `pub struct OllamaExtractorConfig`
- **`class:src/extractor.rs:TokenProvider`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `pub trait TokenProvider: Send + Sync`
- **`const:src/extractor.rs:EXTRACTION_PROMPT`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `const EXTRACTION_PROMPT: &str = r#"You are a memory extraction system. Extract key facts from the following conversation that are worth remembering long-term.

Rules:
- Extract concrete facts, preferences, decisions, and commitments
- Each fact should have a self-contained core_fact (understandable without context)
- Fill dimensional fields ONLY if the information is explicitly present — do NOT infer or fabricate
- Skip greetings, filler, acknowledgments
- Rate importance 0.0-1.0 (preferences=0.6, decisions=0.8, commitments=0.9)
- Rate confidence: "confident" (direct statement), "likely" (reasonable inference), "uncertain" (vague mention)
- If nothing worth remembering, return`
- **`func:src/extractor.rs:deserialize_flexible_string`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn deserialize_flexible_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde_json::Value;

    let value: Option<Value> = Option::deserialize(deserializer)?;
    let value = match value`
- **`func:src/extractor.rs:default_importance`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn default_importance() -> f64`
- **`func:src/extractor.rs:default_confidence`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn default_confidence() -> String`
- **`func:src/extractor.rs:default_domain`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn default_domain() -> String`
- **`func:src/extractor.rs:parse_extraction_response`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn parse_extraction_response(content: &str) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>>`
- **`func:src/extractor.rs:tests::test_parse_new_dimensional_format`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_new_dimensional_format()`
- **`func:src/extractor.rs:tests::test_parse_new_format_array_without_wrapper`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_new_format_array_without_wrapper()`
- **`func:src/extractor.rs:tests::test_parse_markdown_wrapped_new_format`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_markdown_wrapped_new_format()`
- **`func:src/extractor.rs:tests::test_parse_legacy_format`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_legacy_format()`
- **`func:src/extractor.rs:tests::test_parse_legacy_with_surrounding_text`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_legacy_with_surrounding_text()`
- **`func:src/extractor.rs:tests::test_parse_empty_array`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_empty_array()`
- **`func:src/extractor.rs:tests::test_parse_empty_memories`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_empty_memories()`
- **`func:src/extractor.rs:tests::test_parse_invalid_json`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_invalid_json()`
- **`func:src/extractor.rs:tests::test_parse_clamps_importance`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_clamps_importance()`
- **`func:src/extractor.rs:tests::test_parse_filters_empty_core_fact`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_filters_empty_core_fact()`
- **`func:src/extractor.rs:tests::test_parse_legacy_filters_empty`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_legacy_filters_empty()`
- **`func:src/extractor.rs:tests::test_parse_multiple_dimensional_facts`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_multiple_dimensional_facts()`
- **`func:src/extractor.rs:tests::test_parse_all_dimensions`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_parse_all_dimensions()`
- **`func:src/extractor.rs:tests::test_extraction_prompt_format`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_extraction_prompt_format()`
- **`func:src/extractor.rs:tests::test_default_extracted_fact`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_default_extracted_fact()`
- **`func:src/extractor.rs:tests::test_flexible_dim_accepts_empty_array`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_flexible_dim_accepts_empty_array()`
- **`func:src/extractor.rs:tests::test_flexible_dim_accepts_null`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_flexible_dim_accepts_null()`
- **`func:src/extractor.rs:tests::test_flexible_dim_accepts_empty_string`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_flexible_dim_accepts_empty_string()`
- **`func:src/extractor.rs:tests::test_flexible_dim_accepts_single_string`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_flexible_dim_accepts_single_string()`
- **`func:src/extractor.rs:tests::test_flexible_dim_accepts_string_array`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_flexible_dim_accepts_string_array()`
- **`func:src/extractor.rs:tests::test_flexible_dim_filters_empty_items_in_array`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_flexible_dim_filters_empty_items_in_array()`
- **`func:src/extractor.rs:tests::test_flexible_dim_mixed_forms_in_single_payload`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_flexible_dim_mixed_forms_in_single_payload()`
- **`func:src/extractor.rs:tests::test_ollama_extraction`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_ollama_extraction()`
- **`func:src/extractor.rs:tests::test_anthropic_extraction`** (`src/extractor.rs`) — defined_in | score: 0.76
  Sig: `fn test_anthropic_extraction()`
- **`class:src/enriched.rs:ConstructionError`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `pub enum ConstructionError`
- **`class:src/enriched.rs:EnrichedMemory`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `pub struct EnrichedMemory`
- **`func:src/enriched.rs:parse_temporal_mark`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `pub fn parse_temporal_mark(s: &str) -> TemporalMark`
- **`func:src/enriched.rs:precompute_embeddings`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `pub fn precompute_embeddings(
    provider: &EmbeddingProvider,
    items: &mut [EnrichedMemory],
) -> Result<usize, EmbeddingError>`
- **`func:src/enriched.rs:tests::sample_fact`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn sample_fact(core: &str) -> ExtractedFact`
- **`func:src/enriched.rs:tests::from_extracted_round_trip`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn from_extracted_round_trip()`
- **`func:src/enriched.rs:tests::from_extracted_rejects_empty_core_fact`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn from_extracted_rejects_empty_core_fact()`
- **`func:src/enriched.rs:tests::from_extracted_rejects_whitespace_only_core_fact`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn from_extracted_rejects_whitespace_only_core_fact()`
- **`func:src/enriched.rs:tests::from_extracted_clamps_out_of_range_importance`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn from_extracted_clamps_out_of_range_importance()`
- **`func:src/enriched.rs:tests::from_extracted_unknown_domain_becomes_other`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn from_extracted_unknown_domain_becomes_other()`
- **`func:src/enriched.rs:tests::from_extracted_unknown_confidence_becomes_uncertain`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn from_extracted_unknown_confidence_becomes_uncertain()`
- **`func:src/enriched.rs:tests::from_extracted_valence_nan_is_zero`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn from_extracted_valence_nan_is_zero()`
- **`func:src/enriched.rs:tests::minimal_roundtrip`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn minimal_roundtrip()`
- **`func:src/enriched.rs:tests::minimal_rejects_empty`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn minimal_rejects_empty()`
- **`func:src/enriched.rs:tests::from_dimensions_syncs_content`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn from_dimensions_syncs_content()`
- **`func:src/enriched.rs:tests::with_embedding_attaches`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn with_embedding_attaches()`
- **`func:src/enriched.rs:tests::parse_temporal_rfc3339_becomes_exact`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn parse_temporal_rfc3339_becomes_exact()`
- **`func:src/enriched.rs:tests::parse_temporal_naive_datetime_becomes_exact`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn parse_temporal_naive_datetime_becomes_exact()`
- **`func:src/enriched.rs:tests::parse_temporal_date_becomes_day`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn parse_temporal_date_becomes_day()`
- **`func:src/enriched.rs:tests::parse_temporal_unparseable_becomes_vague_lossless`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn parse_temporal_unparseable_becomes_vague_lossless()`
- **`func:src/enriched.rs:tests::parse_temporal_whitespace_is_vague_preserving_input`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn parse_temporal_whitespace_is_vague_preserving_input()`
- **`func:src/enriched.rs:tests::enriched_memory_serde_round_trip`** (`src/enriched.rs`) — defined_in | score: 0.76
  Sig: `fn enriched_memory_serde_round_trip()`
- **`class:src/bus/mod.rs:EmotionalAccumulator`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `pub type EmotionalAccumulator<'a> = EmpathyAccumulator<'a>;
pub type EmotionalTrend = EmpathyTrend;
pub use alignment::{score_alignment, calculate_importance_boost, find_aligned_drives, score_alignment_hybrid, DriveEmbeddings, ALIGNMENT_BOOST};
pub use feedback::{BehaviorFeedback, ActionStats, BehaviorLog, LOW_SCORE_THRESHOLD, MIN_ATTEMPTS_FOR_SUGGESTION};
pub use mod_io::{Drive, HeartbeatTask, Identity};
pub use subscriptions::{SubscriptionManager, Subscription, Notification};

/// A suggested update to SOUL.md based on empathy trends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulUpdate`
- **`class:src/bus/mod.rs:EmotionalBus`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `pub type EmotionalBus = EmpathyBus;

impl EmpathyBus`
- **`class:src/bus/mod.rs:EmotionalTrend`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `pub type EmotionalTrend = EmpathyTrend;
pub use alignment::{score_alignment, calculate_importance_boost, find_aligned_drives, score_alignment_hybrid, DriveEmbeddings, ALIGNMENT_BOOST};
pub use feedback::{BehaviorFeedback, ActionStats, BehaviorLog, LOW_SCORE_THRESHOLD, MIN_ATTEMPTS_FOR_SUGGESTION};
pub use mod_io::{Drive, HeartbeatTask, Identity};
pub use subscriptions::{SubscriptionManager, Subscription, Notification};

/// A suggested update to SOUL.md based on empathy trends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulUpdate`
- **`class:src/bus/mod.rs:EmpathyBus`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `pub struct EmpathyBus`
- **`class:src/bus/mod.rs:HeartbeatUpdate`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `pub struct HeartbeatUpdate`
- **`class:src/bus/mod.rs:SoulUpdate`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `pub struct SoulUpdate`
- **`func:src/bus/mod.rs:tests::setup_workspace`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `fn setup_workspace() -> (TempDir, Connection)`
- **`func:src/bus/mod.rs:tests::test_bus_creation_and_drives`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `fn test_bus_creation_and_drives()`
- **`func:src/bus/mod.rs:tests::test_importance_alignment`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `fn test_importance_alignment()`
- **`func:src/bus/mod.rs:tests::test_process_interaction`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `fn test_process_interaction()`
- **`func:src/bus/mod.rs:tests::test_behavior_logging`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `fn test_behavior_logging()`
- **`func:src/bus/mod.rs:tests::test_suggest_soul_updates`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `fn test_suggest_soul_updates()`
- **`func:src/bus/mod.rs:tests::test_suggest_heartbeat_updates`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `fn test_suggest_heartbeat_updates()`
- **`func:src/bus/mod.rs:tests::test_get_identity`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `fn test_get_identity()`
- **`func:src/bus/mod.rs:tests::test_get_heartbeat_tasks`** (`src/bus/mod.rs`) — defined_in | score: 0.76
  Sig: `fn test_get_heartbeat_tasks()`
- **`infer:component:0.11`** — contains | score: 0.76
  Defines confidence scoring, dimension access patterns, and library root exports.
- **`class:src/bus/accumulator.rs:EmpathyAccumulator`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `pub struct EmpathyAccumulator<'a>`
- **`class:src/bus/accumulator.rs:EmpathyTrend`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `pub struct EmpathyTrend`
- **`const:src/bus/accumulator.rs:MIN_EVENTS_FOR_SUGGESTION`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `pub const MIN_EVENTS_FOR_SUGGESTION: i32 = 10;

/// Empathy trend for a domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmpathyTrend`
- **`const:src/bus/accumulator.rs:NEGATIVE_THRESHOLD`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `pub const NEGATIVE_THRESHOLD: f64 = -0.5;
/// Minimum event count before suggesting SOUL updates.
pub const MIN_EVENTS_FOR_SUGGESTION: i32 = 10;

/// Empathy trend for a domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmpathyTrend`
- **`func:src/bus/accumulator.rs:f64_to_datetime`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `fn f64_to_datetime(ts: f64) -> DateTime<Utc>`
- **`func:src/bus/accumulator.rs:now_f64`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `fn now_f64() -> f64`
- **`func:src/bus/accumulator.rs:tests::test_record_and_get_emotion`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `fn test_record_and_get_emotion()`
- **`func:src/bus/accumulator.rs:tests::test_negative_trend_flags_update`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `fn test_negative_trend_flags_update()`
- **`func:src/bus/accumulator.rs:tests::test_get_all_trends`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `fn test_get_all_trends()`
- **`func:src/bus/accumulator.rs:tests::test_reset_trend`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `fn test_reset_trend()`
- **`func:src/bus/accumulator.rs:tests::test_valence_clamping`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `fn test_valence_clamping()`
- **`func:src/bus/accumulator.rs:tests::test_to_signal_positive_trend`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `fn test_to_signal_positive_trend()`
- **`func:src/bus/accumulator.rs:tests::test_to_signal_negative_trend`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `fn test_to_signal_negative_trend()`
- **`func:src/bus/accumulator.rs:tests::test_to_signal_no_data`** (`src/bus/accumulator.rs`) — defined_in | score: 0.76
  Sig: `fn test_to_signal_no_data()`
- **`infer:component:0.5`** — contains | score: 0.76
  Accumulator and feedback mechanisms for the message bus subsystem.
- **`class:src/entities.rs:EntityConfig`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `pub struct EntityConfig`
- **`class:src/entities.rs:EntityExtractor`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `pub struct EntityExtractor`
- **`class:src/entities.rs:EntityPattern`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `struct EntityPattern`
- **`class:src/entities.rs:EntityType`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `pub enum EntityType`
- **`class:src/entities.rs:ExtractedEntity`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `pub struct ExtractedEntity`
- **`func:src/entities.rs:default_enabled`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn default_enabled() -> bool`
- **`func:src/entities.rs:default_recall_weight`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn default_recall_weight() -> f64`
- **`func:src/entities.rs:normalize_entity_name`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `pub fn normalize_entity_name(name: &str, entity_type: &EntityType) -> String`
- **`func:src/entities.rs:tests::test_entity_type_as_str`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_entity_type_as_str()`
- **`func:src/entities.rs:tests::test_entity_config_default`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_entity_config_default()`
- **`func:src/entities.rs:tests::test_known_entity_extraction`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_known_entity_extraction()`
- **`func:src/entities.rs:tests::test_regex_patterns`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_regex_patterns()`
- **`func:src/entities.rs:tests::test_normalize_entity_name`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_normalize_entity_name()`
- **`func:src/entities.rs:tests::test_dedup`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_dedup()`
- **`func:src/entities.rs:tests::test_case_insensitive_known`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_case_insensitive_known()`
- **`func:src/entities.rs:tests::test_extract_concept_patterns`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_extract_concept_patterns()`
- **`func:src/entities.rs:tests::test_extract_file_paths`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_extract_file_paths()`
- **`func:src/entities.rs:tests::test_extract_urls`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_extract_urls()`
- **`func:src/entities.rs:tests::test_extract_at_mentions`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_extract_at_mentions()`
- **`func:src/entities.rs:tests::test_extract_crate_names`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_extract_crate_names()`
- **`func:src/entities.rs:tests::test_empty_content`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_empty_content()`
- **`func:src/entities.rs:tests::test_dedup_same_entity_twice`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_dedup_same_entity_twice()`
- **`func:src/entities.rs:tests::test_overlapping_known_and_regex`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_overlapping_known_and_regex()`
- **`func:src/entities.rs:tests::test_case_insensitive_known_project`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_case_insensitive_known_project()`
- **`func:src/entities.rs:tests::test_normalize_entity_name_cases`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_normalize_entity_name_cases()`
- **`func:src/entities.rs:tests::test_at_mention_rejects_short_and_numeric`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_at_mention_rejects_short_and_numeric()`
- **`func:src/entities.rs:tests::test_builtin_technologies`** (`src/entities.rs`) — defined_in | score: 0.76
  Sig: `fn test_builtin_technologies()`

context: 150 visited, 150 included, 0 filtered, 3600/15000 tokens, 1ms
