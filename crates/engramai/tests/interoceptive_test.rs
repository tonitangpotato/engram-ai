//! Integration tests for the Interoceptive Layer.
//!
//! Tests the full signal flow: adapter → hub → state → regulation,
//! multi-domain interactions, sliding window trends, and Memory API.

use engramai::interoceptive::{
    hub::InteroceptiveHub,
    regulation::{evaluate, RegulationConfig},
    types::{InteroceptiveSignal, RegulationAction, SignalSource},
};
use engramai::Memory;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Integration: full signal flow (anomaly → hub → state → regulation action)
// ---------------------------------------------------------------------------

#[test]
fn full_signal_flow_anomaly_to_regulation() {
    let mut hub = InteroceptiveHub::new();
    let config = RegulationConfig::default();

    // Phase 1: Feed a stream of negative anomaly signals into "coding" domain.
    for _ in 0..8 {
        let sig = InteroceptiveSignal::new(
            SignalSource::Anomaly,
            Some("coding".into()),
            -0.6,
            0.7,
        );
        hub.process_signal(sig);
    }

    // Phase 2: Check integrated state.
    let state = hub.current_state();
    let ds = state.domain_states.get("coding").unwrap();
    assert!(
        ds.valence_trend < -0.3,
        "valence_trend should be negative after negative signals, got {}",
        ds.valence_trend
    );
    assert!(
        ds.signal_count >= 5,
        "should have enough signals for regulation, got {}",
        ds.signal_count
    );

    // Phase 3: Regulation should generate SoulUpdateSuggestion.
    let actions = evaluate(&state, &config);
    assert!(
        actions.iter().any(|a| matches!(
            a,
            RegulationAction::SoulUpdateSuggestion { domain, .. } if domain == "coding"
        )),
        "expected SoulUpdateSuggestion for coding, got: {:?}",
        actions
    );
}

#[test]
fn full_signal_flow_low_confidence_to_retrieval_adjustment() {
    let mut hub = InteroceptiveHub::new();
    let config = RegulationConfig::default();

    // Simulate low-confidence recall signals in "research" domain.
    for _ in 0..6 {
        let sig = InteroceptiveSignal::new(
            SignalSource::Confidence,
            Some("research".into()),
            -0.3, // low confidence → slightly negative
            0.4,
        );
        hub.process_signal(sig);
    }

    let state = hub.current_state();
    let ds = state.domain_states.get("research").unwrap();
    // Confidence should be low (EWMA tracks valence component mapped to confidence).
    assert!(ds.confidence < 0.5, "confidence should be low, got {}", ds.confidence);

    let actions = evaluate(&state, &config);
    assert!(
        actions.iter().any(|a| matches!(
            a,
            RegulationAction::RetrievalAdjustment { expand_search: true, .. }
        )),
        "expected RetrievalAdjustment, got: {:?}",
        actions
    );
}

// ---------------------------------------------------------------------------
// Integration: multi-domain crossover
// ---------------------------------------------------------------------------

#[test]
fn multi_domain_simultaneous_signals() {
    let mut hub = InteroceptiveHub::new();
    let config = RegulationConfig::default();

    // Coding domain: positive trend.
    for _ in 0..10 {
        hub.process_signal(InteroceptiveSignal::new(
            SignalSource::Feedback,
            Some("coding".into()),
            0.7,
            0.2,
        ));
    }

    // Trading domain: negative trend with anomalies.
    for _ in 0..10 {
        hub.process_signal(InteroceptiveSignal::new(
            SignalSource::Anomaly,
            Some("trading".into()),
            -0.6,
            0.8,
        ));
    }

    let state = hub.current_state();

    // Verify domain isolation.
    let coding = state.domain_states.get("coding").unwrap();
    let trading = state.domain_states.get("trading").unwrap();

    assert!(
        coding.valence_trend > 0.3,
        "coding should be positive, got {}",
        coding.valence_trend
    );
    assert!(
        trading.valence_trend < -0.3,
        "trading should be negative, got {}",
        trading.valence_trend
    );

    // Trading anomaly should trigger an alert.
    let actions = evaluate(&state, &config);
    assert!(
        actions.iter().any(|a| matches!(a, RegulationAction::Alert { .. })),
        "expected anomaly alert for trading, got: {:?}",
        actions
    );

    // Coding should not trigger any negative actions.
    assert!(
        !actions.iter().any(|a| matches!(
            a,
            RegulationAction::SoulUpdateSuggestion { domain, .. } if domain == "coding"
        )),
        "coding is positive, should have no soul update suggestion"
    );

    // Global arousal should be elevated (trading signals are high arousal).
    assert!(
        state.global_arousal > 0.3,
        "global arousal should be elevated, got {}",
        state.global_arousal
    );
}

// ---------------------------------------------------------------------------
// Integration: sliding window trend detection
// ---------------------------------------------------------------------------

#[test]
fn sliding_window_trend_shift() {
    let mut hub = InteroceptiveHub::with_capacity(20, 50, 0.3);

    // Phase 1: positive trend.
    for _ in 0..15 {
        hub.process_signal(InteroceptiveSignal::new(
            SignalSource::Accumulator,
            Some("mood".into()),
            0.6,
            0.2,
        ));
    }

    let state_positive = hub.current_state();
    let mood_pos = state_positive.domain_states.get("mood").unwrap();
    assert!(
        mood_pos.valence_trend > 0.3,
        "should be positive after positive signals, got {}",
        mood_pos.valence_trend
    );

    // Phase 2: shift to negative.
    for _ in 0..30 {
        hub.process_signal(InteroceptiveSignal::new(
            SignalSource::Accumulator,
            Some("mood".into()),
            -0.8,
            0.5,
        ));
    }

    let state_negative = hub.current_state();
    let mood_neg = state_negative.domain_states.get("mood").unwrap();
    assert!(
        mood_neg.valence_trend < -0.3,
        "EWMA should have shifted to negative, got {}",
        mood_neg.valence_trend
    );

    // Buffer should be capped at 20 (the capacity we set).
    assert_eq!(
        state_negative.buffer_size, 20,
        "buffer should be capped at capacity"
    );
}

// ---------------------------------------------------------------------------
// Integration: Memory.interoceptive_snapshot() end-to-end
// ---------------------------------------------------------------------------

#[test]
fn memory_interoceptive_snapshot_end_to_end() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    // Initially, snapshot should be empty (no data yet).
    let initial = mem.interoceptive_snapshot();
    assert!(
        initial.domain_states.is_empty(),
        "initial snapshot should have no domain data"
    );

    // Record some emotional data so interoceptive_tick can pull it.
    {
        let conn = mem.connection();
        let acc = engramai::bus::accumulator::EmotionalAccumulator::new(conn).unwrap();
        for _ in 0..5 {
            acc.record_emotion("coding", 0.7).unwrap();
        }
        for _ in 0..5 {
            acc.record_emotion("trading", -0.5).unwrap();
        }
    }

    // Run interoceptive tick to pull signals from subsystems.
    mem.interoceptive_tick();

    // Snapshot should now reflect domain states.
    let snap = mem.interoceptive_snapshot();
    assert!(
        !snap.domain_states.is_empty(),
        "snapshot should have domain data after tick, got {:?}",
        snap.domain_states
    );

    // Verify the prompt section output is non-trivial.
    let prompt = snap.to_prompt_section();
    assert!(
        prompt != "Internal state: no data yet.",
        "prompt should have content after tick"
    );
}

#[test]
fn memory_interoceptive_tick_pulls_feedback() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    // Record behavior feedback.
    {
        let conn = mem.connection();
        let fb = engramai::bus::feedback::BehaviorFeedback::new(conn).unwrap();
        for _ in 0..8 {
            fb.log_outcome("web_search", true).unwrap();
        }
        for _ in 0..2 {
            fb.log_outcome("web_search", false).unwrap();
        }
    }

    mem.interoceptive_tick();

    // Hub should have processed feedback signals.
    let hub = mem.interoceptive_hub();
    assert!(
        hub.buffer_len() > 0,
        "hub should have processed feedback signals"
    );
}

// ---------------------------------------------------------------------------
// Integration: somatic markers across signal types
// ---------------------------------------------------------------------------

#[test]
fn somatic_markers_accumulate_across_encounters() {
    let mut hub = InteroceptiveHub::new();

    // First encounter with memory 42: positive.
    let m1 = hub.somatic_lookup(42, 0.8);
    assert_eq!(m1.encounter_count, 1);
    assert!((m1.valence - 0.8).abs() < f64::EPSILON);

    // Second encounter: negative.
    let m2 = hub.somatic_lookup(42, -0.4);
    assert_eq!(m2.encounter_count, 2);
    // Mean of 0.8 and -0.4 = 0.2.
    assert!((m2.valence - 0.2).abs() < 0.01);

    // Different memory.
    let other = hub.somatic_lookup(99, -1.0);
    assert_eq!(other.encounter_count, 1);

    // First memory is unaffected.
    let m3 = hub.somatic_lookup(42, 0.2);
    assert_eq!(m3.encounter_count, 3);
}

// ---------------------------------------------------------------------------
// Integration: mixed signal sources in a single batch
// ---------------------------------------------------------------------------

#[test]
fn mixed_signal_batch_processing() {
    let mut hub = InteroceptiveHub::new();

    let signals = vec![
        // Anomaly → coding
        InteroceptiveSignal::new(SignalSource::Anomaly, Some("coding".into()), -0.5, 0.7),
        // Accumulator → coding (same domain, different source)
        InteroceptiveSignal::new(SignalSource::Accumulator, Some("coding".into()), 0.3, 0.2),
        // Feedback → global
        InteroceptiveSignal::new(SignalSource::Feedback, None, 0.6, 0.1),
        // Confidence → research
        InteroceptiveSignal::new(SignalSource::Confidence, Some("research".into()), 0.4, 0.1),
        // Alignment → global
        InteroceptiveSignal::new(SignalSource::Alignment, None, 0.7, 0.15),
    ];

    let _notable = hub.process_batch(signals);

    // Should have 3 domains: coding, _global, research.
    assert_eq!(hub.domain_count(), 3, "expected 3 domains");
    assert_eq!(hub.buffer_len(), 5, "all 5 signals in buffer");

    // Coding got mixed signals (negative + positive).
    let coding = hub.domain_state("coding").unwrap();
    assert_eq!(coding.signal_count, 2);

    // Global got feedback + alignment.
    let global = hub.domain_state("_global").unwrap();
    assert_eq!(global.signal_count, 2);
}

// ---------------------------------------------------------------------------
// Integration: prompt section generation reflects state
// ---------------------------------------------------------------------------

#[test]
fn prompt_section_reflects_domain_state() {
    let mut hub = InteroceptiveHub::new();

    // Build up a strongly negative domain.
    for _ in 0..10 {
        hub.process_signal(InteroceptiveSignal::new(
            SignalSource::Anomaly,
            Some("debugging".into()),
            -0.7,
            0.8,
        ));
    }

    let state = hub.current_state();
    let prompt = state.to_prompt_section();

    // Should mention the domain and its negative state.
    assert!(prompt.contains("debugging"), "prompt should mention domain");
    assert!(
        prompt.contains("negative") || prompt.contains("stressed"),
        "prompt should describe negative valence: {}",
        prompt
    );
}
