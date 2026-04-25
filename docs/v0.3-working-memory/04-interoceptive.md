# Context Assembly

**Tokens**: 4492/12000 | **Nodes**: 148 visited, 148 included, 0 filtered
**Elapsed**: 1ms

## Targets

### `file:src/interoceptive/hub.rs` — hub.rs
**File**: `src/interoceptive/hub.rs`
*~14 tokens*

### `file:src/interoceptive/regulation.rs` — regulation.rs
**File**: `src/interoceptive/regulation.rs`
*~15 tokens*

### `file:src/interoceptive/types.rs` — types.rs
**File**: `src/interoceptive/types.rs`
*~14 tokens*

### `file:src/anomaly.rs` — anomaly.rs
**File**: `src/anomaly.rs`
*~15 tokens*

### `file:src/confidence.rs` — confidence.rs
**File**: `src/confidence.rs`
*~15 tokens*

## Dependencies

- **`module:src/interoceptive`** (`src/interoceptive`) — belongs_to | score: 0.52
- **`module:src`** (`src`) — belongs_to | score: 0.52
## Callers

- **`class:src/interoceptive/hub.rs:BaselineKey`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `type BaselineKey = (String, String); // (source_name, domain)

/// Default minimum samples before baseline is calibrated.
const DEFAULT_BASELINE_MIN_SAMPLES: u64 = 20;

/// The central integration hub for interoceptive signals.
///
/// Analogous to the anterior insula in Craig's model — receives raw
/// interoceptive signals and builds an integrated "feeling state."
pub struct InteroceptiveHub`
- **`class:src/interoceptive/hub.rs:InteroceptiveHub`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `pub struct InteroceptiveHub`
- **`const:src/interoceptive/hub.rs:DEFAULT_ALPHA`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `const DEFAULT_ALPHA: f64 = 0.3;

/// Composite key for per-source-per-domain baselines.
type BaselineKey = (String, String); // (source_name, domain)

/// Default minimum samples before baseline is calibrated.
const DEFAULT_BASELINE_MIN_SAMPLES: u64 = 20;

/// The central integration hub for interoceptive signals.
///
/// Analogous to the anterior insula in Craig's model — receives raw
/// interoceptive signals and builds an integrated "feeling state."
pub struct InteroceptiveHub`
- **`const:src/interoceptive/hub.rs:DEFAULT_BASELINE_MIN_SAMPLES`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `const DEFAULT_BASELINE_MIN_SAMPLES: u64 = 20;

/// The central integration hub for interoceptive signals.
///
/// Analogous to the anterior insula in Craig's model — receives raw
/// interoceptive signals and builds an integrated "feeling state."
pub struct InteroceptiveHub`
- **`const:src/interoceptive/hub.rs:DEFAULT_BUFFER_CAPACITY`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `const DEFAULT_BUFFER_CAPACITY: usize = 1000;

/// Default maximum number of somatic markers to cache.
const DEFAULT_MARKER_CACHE_SIZE: usize = 256;

/// Default EWMA alpha (recency weight). 0.3 gives ~70% weight to history.
const DEFAULT_ALPHA: f64 = 0.3;

/// Composite key for per-source-per-domain baselines.
type BaselineKey = (String, String); // (source_name, domain)

/// Default minimum samples before baseline is calibrated.
const DEFAULT_BASELINE_MIN_SAMPLES: u64 = 20;

/// The central integration hub for interoceptive signals.
///
/// Analogous to the anterior insula in Craig's model — receives raw
/// interoceptive signals and builds an integrated "feeling state."
pub struct InteroceptiveHub`
- **`const:src/interoceptive/hub.rs:DEFAULT_MARKER_CACHE_SIZE`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `const DEFAULT_MARKER_CACHE_SIZE: usize = 256;

/// Default EWMA alpha (recency weight). 0.3 gives ~70% weight to history.
const DEFAULT_ALPHA: f64 = 0.3;

/// Composite key for per-source-per-domain baselines.
type BaselineKey = (String, String); // (source_name, domain)

/// Default minimum samples before baseline is calibrated.
const DEFAULT_BASELINE_MIN_SAMPLES: u64 = 20;

/// The central integration hub for interoceptive signals.
///
/// Analogous to the anterior insula in Craig's model — receives raw
/// interoceptive signals and builds an integrated "feeling state."
pub struct InteroceptiveHub`
- **`func:src/interoceptive/hub.rs:tests::hub_processes_signal_and_updates_domain`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `fn hub_processes_signal_and_updates_domain()`
- **`func:src/interoceptive/hub.rs:tests::hub_notable_signal`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `fn hub_notable_signal()`
- **`func:src/interoceptive/hub.rs:tests::hub_buffer_fifo_eviction`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `fn hub_buffer_fifo_eviction()`
- **`func:src/interoceptive/hub.rs:tests::hub_global_arousal_computation`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `fn hub_global_arousal_computation()`
- **`func:src/interoceptive/hub.rs:tests::hub_somatic_marker_creation_and_update`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `fn hub_somatic_marker_creation_and_update()`
- **`func:src/interoceptive/hub.rs:tests::hub_somatic_lru_eviction`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `fn hub_somatic_lru_eviction()`
- **`func:src/interoceptive/hub.rs:tests::hub_current_state_snapshot`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `fn hub_current_state_snapshot()`
- **`func:src/interoceptive/hub.rs:tests::hub_global_domain_signal`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `fn hub_global_domain_signal()`
- **`func:src/interoceptive/hub.rs:tests::hub_process_batch`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `fn hub_process_batch()`
- **`func:src/interoceptive/hub.rs:tests::hub_clear`** (`src/interoceptive/hub.rs`) — defined_in | score: 0.76
  Sig: `fn hub_clear()`
- **`infer:component:infrastructure`** — contains | score: 0.76
  Core platform infrastructure including compiler, storage, discovery, embeddings, and event bus systems.
- **`class:src/interoceptive/regulation.rs:RegulationConfig`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `pub struct RegulationConfig`
- **`func:src/interoceptive/regulation.rs:evaluate`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `pub fn evaluate(state: &InteroceptiveState, config: &RegulationConfig) -> Vec<RegulationAction>`
- **`func:src/interoceptive/regulation.rs:evaluate_with_hub`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `pub fn evaluate_with_hub(
    state: &InteroceptiveState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
) -> Vec<RegulationAction>`
- **`func:src/interoceptive/regulation.rs:check_valence`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn check_valence(
    ds: &DomainState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
)`
- **`func:src/interoceptive/regulation.rs:check_confidence`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn check_confidence(
    ds: &DomainState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
)`
- **`func:src/interoceptive/regulation.rs:check_success_rate`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn check_success_rate(
    ds: &DomainState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
)`
- **`func:src/interoceptive/regulation.rs:check_alignment`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn check_alignment(
    ds: &DomainState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
)`
- **`func:src/interoceptive/regulation.rs:check_identity_evolution`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn check_identity_evolution(
    state: &InteroceptiveState,
    _config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
)`
- **`func:src/interoceptive/regulation.rs:check_multi_domain_anomaly`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn check_multi_domain_anomaly(
    state: &InteroceptiveState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
)`
- **`func:src/interoceptive/regulation.rs:check_heartbeat_frequency`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn check_heartbeat_frequency(
    state: &InteroceptiveState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
)`
- **`func:src/interoceptive/regulation.rs:tests::make_state`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn make_state(domains: Vec<DomainState>) -> InteroceptiveState`
- **`func:src/interoceptive/regulation.rs:tests::domain_with`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn domain_with(
        name: &str,
        valence: f64,
        confidence: f64,
        success: f64,
        alignment: f64,
        anomaly: f64,
        signals: u64,
    ) -> DomainState`
- **`func:src/interoceptive/regulation.rs:tests::no_actions_for_healthy_state`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn no_actions_for_healthy_state()`
- **`func:src/interoceptive/regulation.rs:tests::fallback_negative_valence_triggers_soul_update`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn fallback_negative_valence_triggers_soul_update()`
- **`func:src/interoceptive/regulation.rs:tests::fallback_needs_min_signals`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn fallback_needs_min_signals()`
- **`func:src/interoceptive/regulation.rs:tests::fallback_low_confidence_triggers_retrieval`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn fallback_low_confidence_triggers_retrieval()`
- **`func:src/interoceptive/regulation.rs:tests::fallback_low_success_triggers_behavior_shift`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn fallback_low_success_triggers_behavior_shift()`
- **`func:src/interoceptive/regulation.rs:tests::fallback_low_alignment_triggers_soul_update`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn fallback_low_alignment_triggers_soul_update()`
- **`func:src/interoceptive/regulation.rs:tests::fallback_multi_domain_anomaly_triggers_high_alert`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn fallback_multi_domain_anomaly_triggers_high_alert()`
- **`func:src/interoceptive/regulation.rs:tests::build_calibrated_hub`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn build_calibrated_hub(domain: &str, normal_valence: f64, n: u64) -> InteroceptiveHub`
- **`func:src/interoceptive/regulation.rs:tests::adaptive_no_action_when_within_baseline`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn adaptive_no_action_when_within_baseline()`
- **`func:src/interoceptive/regulation.rs:tests::adaptive_triggers_on_large_deviation`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn adaptive_triggers_on_large_deviation()`
- **`func:src/interoceptive/regulation.rs:tests::adaptive_message_includes_sigma`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn adaptive_message_includes_sigma()`
- **`func:src/interoceptive/regulation.rs:tests::adaptive_falls_back_when_uncalibrated`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn adaptive_falls_back_when_uncalibrated()`
- **`func:src/interoceptive/regulation.rs:tests::multiple_issues_generate_multiple_actions`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn multiple_issues_generate_multiple_actions()`
- **`func:src/interoceptive/regulation.rs:tests::build_high_performance_hub`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn build_high_performance_hub(domain: &str, n: u64) -> InteroceptiveHub`
- **`func:src/interoceptive/regulation.rs:tests::identity_evolution_not_triggered_without_hub`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn identity_evolution_not_triggered_without_hub()`
- **`func:src/interoceptive/regulation.rs:tests::identity_evolution_not_triggered_with_few_signals`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn identity_evolution_not_triggered_with_few_signals()`
- **`func:src/interoceptive/regulation.rs:tests::identity_evolution_triggers_domain_specialization`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn identity_evolution_triggers_domain_specialization()`
- **`func:src/interoceptive/regulation.rs:tests::identity_evolution_triggers_capability`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn identity_evolution_triggers_capability()`
- **`func:src/interoceptive/regulation.rs:tests::identity_evolution_triggers_behavioral_pattern`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn identity_evolution_triggers_behavioral_pattern()`
- **`func:src/interoceptive/regulation.rs:tests::identity_evolution_not_triggered_by_mediocre_performance`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn identity_evolution_not_triggered_by_mediocre_performance()`
- **`func:src/interoceptive/regulation.rs:tests::identity_evolution_suggestion_has_valid_confidence`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn identity_evolution_suggestion_has_valid_confidence()`
- **`func:src/interoceptive/regulation.rs:tests::heartbeat_no_adjustment_with_single_domain`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn heartbeat_no_adjustment_with_single_domain()`
- **`func:src/interoceptive/regulation.rs:tests::heartbeat_no_adjustment_with_few_signals`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn heartbeat_no_adjustment_with_few_signals()`
- **`func:src/interoceptive/regulation.rs:tests::heartbeat_increase_when_two_domains_troubled_fallback`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn heartbeat_increase_when_two_domains_troubled_fallback()`
- **`func:src/interoceptive/regulation.rs:tests::heartbeat_increase_with_high_global_arousal`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn heartbeat_increase_with_high_global_arousal()`
- **`func:src/interoceptive/regulation.rs:tests::heartbeat_increase_severe_with_three_troubled_domains`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn heartbeat_increase_severe_with_three_troubled_domains()`
- **`func:src/interoceptive/regulation.rs:tests::build_stable_multi_domain_hub`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn build_stable_multi_domain_hub(domains: &[&str], rounds: usize) -> InteroceptiveHub`
- **`func:src/interoceptive/regulation.rs:tests::heartbeat_decrease_when_all_domains_stable`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn heartbeat_decrease_when_all_domains_stable()`
- **`func:src/interoceptive/regulation.rs:tests::heartbeat_decrease_requires_hub`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn heartbeat_decrease_requires_hub()`
- **`func:src/interoceptive/regulation.rs:tests::heartbeat_no_decrease_when_uncalibrated`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn heartbeat_no_decrease_when_uncalibrated()`
- **`func:src/interoceptive/regulation.rs:tests::heartbeat_no_simultaneous_increase_and_decrease`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn heartbeat_no_simultaneous_increase_and_decrease()`
- **`func:src/interoceptive/regulation.rs:tests::heartbeat_increase_with_adaptive_baselines`** (`src/interoceptive/regulation.rs`) — defined_in | score: 0.76
  Sig: `fn heartbeat_increase_with_adaptive_baselines()`
- **`infer:component:0.14`** — contains | score: 0.76
  Handles internal state regulation and interoceptive processing with associated tests.
- **`class:src/interoceptive/types.rs:AdaptiveBaseline`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub struct AdaptiveBaseline`
- **`class:src/interoceptive/types.rs:AlertSeverity`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub enum AlertSeverity`
- **`class:src/interoceptive/types.rs:DeviationLevel`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub enum DeviationLevel`
- **`class:src/interoceptive/types.rs:DomainState`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub struct DomainState`
- **`class:src/interoceptive/types.rs:HeartbeatAdjustDirection`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub enum HeartbeatAdjustDirection`
- **`class:src/interoceptive/types.rs:IdentityAspect`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub enum IdentityAspect`
- **`class:src/interoceptive/types.rs:InteroceptiveSignal`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub struct InteroceptiveSignal`
- **`class:src/interoceptive/types.rs:InteroceptiveState`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub struct InteroceptiveState`
- **`class:src/interoceptive/types.rs:RegulationAction`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub enum RegulationAction`
- **`class:src/interoceptive/types.rs:SignalContext`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub enum SignalContext`
- **`class:src/interoceptive/types.rs:SignalSource`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub enum SignalSource`
- **`class:src/interoceptive/types.rs:SomaticMarker`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `pub struct SomaticMarker`
- **`func:src/interoceptive/types.rs:tests::signal_clamps_values`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn signal_clamps_values()`
- **`func:src/interoceptive/types.rs:tests::signal_negative_and_urgent`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn signal_negative_and_urgent()`
- **`func:src/interoceptive/types.rs:tests::signal_with_context`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn signal_with_context()`
- **`func:src/interoceptive/types.rs:tests::domain_state_update_ewma`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn domain_state_update_ewma()`
- **`func:src/interoceptive/types.rs:tests::domain_state_source_specific_updates`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn domain_state_source_specific_updates()`
- **`func:src/interoceptive/types.rs:tests::somatic_marker_incremental_mean`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn somatic_marker_incremental_mean()`
- **`func:src/interoceptive/types.rs:tests::interoceptive_state_prompt_output`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn interoceptive_state_prompt_output()`
- **`func:src/interoceptive/types.rs:tests::baseline_uncalibrated_before_min_samples`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn baseline_uncalibrated_before_min_samples()`
- **`func:src/interoceptive/types.rs:tests::baseline_calibrates_at_min_samples`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn baseline_calibrates_at_min_samples()`
- **`func:src/interoceptive/types.rs:tests::baseline_sigma_deviation_correct`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn baseline_sigma_deviation_correct()`
- **`func:src/interoceptive/types.rs:tests::baseline_deviation_levels`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn baseline_deviation_levels()`
- **`func:src/interoceptive/types.rs:tests::baseline_zero_variance_handling`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn baseline_zero_variance_handling()`
- **`func:src/interoceptive/types.rs:tests::baseline_with_decay_adapts`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn baseline_with_decay_adapts()`
- **`func:src/interoceptive/types.rs:tests::deviation_level_actionable`** (`src/interoceptive/types.rs`) — defined_in | score: 0.76
  Sig: `fn deviation_level_actionable()`
- **`class:src/anomaly.rs:AnomalyResult`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `pub struct AnomalyResult`
- **`class:src/anomaly.rs:Baseline`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `pub struct Baseline`
- **`class:src/anomaly.rs:BaselineTracker`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `pub struct BaselineTracker`
- **`const:src/anomaly.rs:DEFAULT_WINDOW_SIZE`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `const DEFAULT_WINDOW_SIZE: usize = 100;

/// Baseline statistics for a single metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline`
- **`func:src/anomaly.rs:tests::test_baseline_calculation`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_baseline_calculation()`
- **`func:src/anomaly.rs:tests::test_z_score`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_z_score()`
- **`func:src/anomaly.rs:tests::test_anomaly_detection`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_anomaly_detection()`
- **`func:src/anomaly.rs:tests::test_min_samples`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_min_samples()`
- **`func:src/anomaly.rs:tests::test_window_eviction`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_window_eviction()`
- **`func:src/anomaly.rs:tests::test_percentile`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_percentile()`
- **`func:src/anomaly.rs:tests::test_high_low_anomaly`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_high_low_anomaly()`
- **`func:src/anomaly.rs:tests::test_analyze`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_analyze()`
- **`func:src/anomaly.rs:tests::test_to_signal_normal_value`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_to_signal_normal_value()`
- **`func:src/anomaly.rs:tests::test_to_signal_anomalous_value`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_to_signal_anomalous_value()`
- **`func:src/anomaly.rs:tests::test_to_signal_no_data_returns_none`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_to_signal_no_data_returns_none()`
- **`func:src/anomaly.rs:tests::test_to_signal_zero_std_returns_none`** (`src/anomaly.rs`) — defined_in | score: 0.76
  Sig: `fn test_to_signal_zero_std_returns_none()`
- **`infer:component:0.6`** — contains | score: 0.76
  Groups the public API interface and its associated test suites for knowledge compilation.
- **`class:src/confidence.rs:ConfidenceDetail`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub struct ConfidenceDetail`
- **`const:src/confidence.rs:confidence_thresholds::CERTAIN`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub const CERTAIN: f64 = 0.8;
    pub const LIKELY: f64 = 0.6;
    pub const UNCERTAIN: f64 = 0.4;
    // Below UNCERTAIN = "vague"
}

/// Detailed confidence breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceDetail`
- **`const:src/confidence.rs:confidence_thresholds::LIKELY`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub const LIKELY: f64 = 0.6;
    pub const UNCERTAIN: f64 = 0.4;
    // Below UNCERTAIN = "vague"
}

/// Detailed confidence breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceDetail`
- **`const:src/confidence.rs:confidence_thresholds::UNCERTAIN`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub const UNCERTAIN: f64 = 0.4;
    // Below UNCERTAIN = "vague"
}

/// Detailed confidence breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceDetail`
- **`const:src/confidence.rs:default_reliability::CAUSAL`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub const CAUSAL: f64 = 0.80;
}

/// Confidence labels with thresholds.
pub mod confidence_thresholds`
- **`const:src/confidence.rs:default_reliability::EMOTIONAL`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub const EMOTIONAL: f64 = 0.95;
    pub const PROCEDURAL: f64 = 0.90;
    pub const OPINION: f64 = 0.60;
    pub const CAUSAL: f64 = 0.80;
}

/// Confidence labels with thresholds.
pub mod confidence_thresholds`
- **`const:src/confidence.rs:default_reliability::EPISODIC`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub const EPISODIC: f64 = 0.90;
    pub const RELATIONAL: f64 = 0.75;
    pub const EMOTIONAL: f64 = 0.95;
    pub const PROCEDURAL: f64 = 0.90;
    pub const OPINION: f64 = 0.60;
    pub const CAUSAL: f64 = 0.80;
}

/// Confidence labels with thresholds.
pub mod confidence_thresholds`
- **`const:src/confidence.rs:default_reliability::FACTUAL`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub const FACTUAL: f64 = 0.85;
    pub const EPISODIC: f64 = 0.90;
    pub const RELATIONAL: f64 = 0.75;
    pub const EMOTIONAL: f64 = 0.95;
    pub const PROCEDURAL: f64 = 0.90;
    pub const OPINION: f64 = 0.60;
    pub const CAUSAL: f64 = 0.80;
}

/// Confidence labels with thresholds.
pub mod confidence_thresholds`
- **`const:src/confidence.rs:default_reliability::OPINION`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub const OPINION: f64 = 0.60;
    pub const CAUSAL: f64 = 0.80;
}

/// Confidence labels with thresholds.
pub mod confidence_thresholds`
- **`const:src/confidence.rs:default_reliability::PROCEDURAL`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub const PROCEDURAL: f64 = 0.90;
    pub const OPINION: f64 = 0.60;
    pub const CAUSAL: f64 = 0.80;
}

/// Confidence labels with thresholds.
pub mod confidence_thresholds`
- **`const:src/confidence.rs:default_reliability::RELATIONAL`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub const RELATIONAL: f64 = 0.75;
    pub const EMOTIONAL: f64 = 0.95;
    pub const PROCEDURAL: f64 = 0.90;
    pub const OPINION: f64 = 0.60;
    pub const CAUSAL: f64 = 0.80;
}

/// Confidence labels with thresholds.
pub mod confidence_thresholds`
- **`func:src/confidence.rs:type_reliability`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub fn type_reliability(memory_type: MemoryType) -> f64`
- **`func:src/confidence.rs:content_reliability`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub fn content_reliability(record: &MemoryRecord) -> f64`
- **`func:src/confidence.rs:retrieval_salience`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub fn retrieval_salience(record: &MemoryRecord, all_records: Option<&[MemoryRecord]>) -> f64`
- **`func:src/confidence.rs:effective_strength`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn effective_strength(record: &MemoryRecord) -> f64`
- **`func:src/confidence.rs:sigmoid_salience`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn sigmoid_salience(strength: f64) -> f64`
- **`func:src/confidence.rs:confidence_score`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub fn confidence_score(record: &MemoryRecord, all_records: Option<&[MemoryRecord]>) -> f64`
- **`func:src/confidence.rs:confidence_label`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub fn confidence_label(score: f64) -> &'static str`
- **`func:src/confidence.rs:confidence_detail`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub fn confidence_detail(
    record: &MemoryRecord,
    all_records: Option<&[MemoryRecord]>,
) -> ConfidenceDetail`
- **`func:src/confidence.rs:batch_confidence`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub fn batch_confidence(records: &[MemoryRecord]) -> Vec<(String, f64, &'static str)>`
- **`func:src/confidence.rs:calibrate_confidence`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub fn calibrate_confidence(
    record: &MemoryRecord,
    activation: f64,
    all_records: &[MemoryRecord],
) -> f64`
- **`func:src/confidence.rs:confidence_to_signal`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `pub fn confidence_to_signal(
    record: &MemoryRecord,
    all_records: Option<&[MemoryRecord]>,
    query: Option<&str>,
) -> crate::interoceptive::InteroceptiveSignal`
- **`func:src/confidence.rs:tests::make_record`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn make_record(
        memory_type: MemoryType,
        importance: f64,
        pinned: bool,
        contradicted: bool,
        working: f64,
        core: f64,
    ) -> MemoryRecord`
- **`func:src/confidence.rs:tests::test_type_reliability`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_type_reliability()`
- **`func:src/confidence.rs:tests::test_content_reliability_basic`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_content_reliability_basic()`
- **`func:src/confidence.rs:tests::test_content_reliability_contradicted`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_content_reliability_contradicted()`
- **`func:src/confidence.rs:tests::test_content_reliability_pinned`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_content_reliability_pinned()`
- **`func:src/confidence.rs:tests::test_retrieval_salience`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_retrieval_salience()`
- **`func:src/confidence.rs:tests::test_confidence_score`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_score()`
- **`func:src/confidence.rs:tests::test_confidence_labels`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_labels()`
- **`func:src/confidence.rs:tests::test_confidence_detail`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_detail()`
- **`func:src/confidence.rs:tests::test_batch_confidence`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_batch_confidence()`
- **`func:src/confidence.rs:tests::test_calibrate_confidence`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_calibrate_confidence()`
- **`func:src/confidence.rs:tests::test_confidence_to_signal_high_confidence`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_to_signal_high_confidence()`
- **`func:src/confidence.rs:tests::test_confidence_to_signal_low_confidence`** (`src/confidence.rs`) — defined_in | score: 0.76
  Sig: `fn test_confidence_to_signal_low_confidence()`
- **`infer:component:0.11`** — contains | score: 0.76
  Defines confidence scoring, dimension access patterns, and library root exports.

context: 148 visited, 148 included, 0 filtered, 4492/12000 tokens, 1ms
