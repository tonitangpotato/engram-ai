//! Integration tests for ISS-019 Step 8: write-path telemetry.
//!
//! Covers:
//! - Default `CountingSink` is installed out of the box; `write_stats()`
//!   returns `Some(stats)` with fresh zero counters on a new `Memory`.
//! - `store_raw` with normal content → `stored_count` increments.
//! - `store_raw` with whitespace content → `skipped_count` increments
//!   and `skipped_by_reason[TooShort]` bumps.
//! - Extractor failure → `quarantined_count` increments.
//! - Extractor returns empty facts → `skipped_by_reason[NoFactsExtracted]`
//!   bumps.
//! - `reset_write_stats()` zeros counters.
//! - Custom sink via `set_event_sink()` replaces the default; the
//!   custom sink observes every event; `write_stats()` returns `None`
//!   while a custom sink is installed.
//! - Batch extractor (≥2 facts) → one Stored event, fact_count matches.
//! - `ms_total` monotonically increases across calls.

use engramai::extractor::{ExtractedFact, MemoryExtractor};
use engramai::memory::Memory;
use engramai::store_api::{QuarantineReason, RawStoreOutcome, StorageMeta, SkipReason};
use engramai::write_stats::{CountingSink, EventSink, SharedSink, StoreEvent};
use std::error::Error as StdError;
use std::sync::{Arc, Mutex};

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

fn default_meta() -> StorageMeta {
    StorageMeta {
        importance_hint: Some(0.5),
        memory_type_hint: None,
        source: Some("test".to_string()),
        namespace: None,
        user_metadata: serde_json::Value::Null,
    }
}

// --------------------------------------------------------------------
// Fake extractors for controlling the store_raw return point
// --------------------------------------------------------------------

type ExtractErr = Box<dyn StdError + Send + Sync>;

struct EmptyExtractor;
impl MemoryExtractor for EmptyExtractor {
    fn extract(&self, _text: &str) -> Result<Vec<ExtractedFact>, ExtractErr> {
        Ok(vec![])
    }
}

struct ErrorExtractor;
impl MemoryExtractor for ErrorExtractor {
    fn extract(&self, _text: &str) -> Result<Vec<ExtractedFact>, ExtractErr> {
        Err("503 upstream timeout".into())
    }
}

struct MultiFactExtractor {
    fact_count: usize,
}
impl MemoryExtractor for MultiFactExtractor {
    fn extract(&self, _text: &str) -> Result<Vec<ExtractedFact>, ExtractErr> {
        let facts = (0..self.fact_count)
            .map(|i| ExtractedFact {
                core_fact: format!("fact number {i}"),
                importance: 0.5,
                valence: 0.0,
                domain: "general".to_string(),
                ..Default::default()
            })
            .collect();
        Ok(facts)
    }
}

// --------------------------------------------------------------------
// Capturing sink — records every event in-order for assertion
// --------------------------------------------------------------------

#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<StoreEvent>>,
}
impl CapturingSink {
    fn len(&self) -> usize {
        self.events.lock().unwrap().len()
    }
    fn last_is_stored(&self) -> bool {
        matches!(
            self.events.lock().unwrap().last(),
            Some(StoreEvent::Stored { .. })
        )
    }
    fn stored_fact_counts(&self) -> Vec<usize> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                StoreEvent::Stored { fact_count, .. } => Some(*fact_count),
                _ => None,
            })
            .collect()
    }
}
impl EventSink for CapturingSink {
    fn record(&self, event: StoreEvent) {
        self.events.lock().unwrap().push(event);
    }
}

// --------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------

#[test]
fn default_counting_sink_is_installed() {
    let mem = new_mem();
    let stats = mem.write_stats().expect("default sink present");
    assert_eq!(stats.total_calls(), 0);
    assert_eq!(stats.stored_count, 0);
    assert_eq!(stats.skipped_count, 0);
    assert_eq!(stats.quarantined_count, 0);
    assert!(stats.skipped_by_reason.is_empty());
}

#[test]
fn store_raw_success_bumps_stored_count() {
    let mut mem = new_mem();
    let _ = mem
        .store_raw("ISS-019 Step 8 telemetry lands today", default_meta())
        .expect("store_raw ok");
    let stats = mem.write_stats().unwrap();
    assert_eq!(stats.stored_count, 1);
    assert_eq!(stats.stored_fact_count, 1);
    assert_eq!(stats.skipped_count, 0);
    assert_eq!(stats.quarantined_count, 0);
    assert_eq!(stats.total_calls(), 1);
}

#[test]
fn whitespace_content_bumps_too_short_bucket() {
    let mut mem = new_mem();
    let out = mem.store_raw("   \n\t  ", default_meta()).expect("ok");
    assert!(matches!(
        out,
        RawStoreOutcome::Skipped {
            reason: SkipReason::TooShort,
            ..
        }
    ));
    let stats = mem.write_stats().unwrap();
    assert_eq!(stats.skipped_count, 1);
    assert_eq!(stats.stored_count, 0);
    assert_eq!(
        stats.skipped_by_reason.get(&SkipReason::TooShort),
        Some(&1)
    );
}

#[test]
fn empty_extractor_result_bumps_no_facts_bucket() {
    let mut mem = new_mem();
    mem.set_extractor(Box::new(EmptyExtractor));
    let out = mem
        .store_raw("some input that the extractor shrugs at", default_meta())
        .expect("ok");
    assert!(matches!(
        out,
        RawStoreOutcome::Skipped {
            reason: SkipReason::NoFactsExtracted,
            ..
        }
    ));
    let stats = mem.write_stats().unwrap();
    assert_eq!(stats.skipped_count, 1);
    assert_eq!(
        stats.skipped_by_reason.get(&SkipReason::NoFactsExtracted),
        Some(&1)
    );
    // TooShort bucket must not be incremented.
    assert_eq!(stats.skipped_by_reason.get(&SkipReason::TooShort), None);
}

#[test]
fn extractor_error_bumps_quarantine_count() {
    let mut mem = new_mem();
    mem.set_extractor(Box::new(ErrorExtractor));
    let out = mem
        .store_raw("input that will blow up the extractor", default_meta())
        .expect("ok");
    assert!(matches!(
        out,
        RawStoreOutcome::Quarantined {
            reason: QuarantineReason::ExtractorError(_),
            ..
        }
    ));
    let stats = mem.write_stats().unwrap();
    assert_eq!(stats.quarantined_count, 1);
    assert_eq!(stats.stored_count, 0);
    assert_eq!(stats.skipped_count, 0);
    assert_eq!(stats.total_calls(), 1);
}

#[test]
fn batch_extractor_emits_single_stored_event_with_fact_count() {
    let mut mem = new_mem();
    mem.set_extractor(Box::new(MultiFactExtractor { fact_count: 3 }));

    // Capturing sink so we can verify "exactly one event, fact_count=3".
    let capture = Arc::new(CapturingSink::default());
    mem.set_event_sink(capture.clone() as SharedSink);

    let _ = mem
        .store_raw(
            "input that produces three facts via our fake extractor",
            default_meta(),
        )
        .expect("ok");

    assert_eq!(capture.len(), 1, "one store_raw call -> one event");
    assert!(capture.last_is_stored(), "batch should produce Stored event");
    assert_eq!(capture.stored_fact_counts(), vec![3]);
}

#[test]
fn reset_write_stats_zeroes_counters() {
    let mut mem = new_mem();
    let _ = mem.store_raw("first entry", default_meta()).unwrap();
    let _ = mem.store_raw("   ", default_meta()).unwrap();
    let before = mem.write_stats().unwrap();
    assert!(before.total_calls() >= 2);

    let reset_did_run = mem.reset_write_stats();
    assert!(reset_did_run);

    let after = mem.write_stats().unwrap();
    assert_eq!(after.total_calls(), 0);
    assert_eq!(after.stored_count, 0);
    assert_eq!(after.skipped_count, 0);
    assert!(after.skipped_by_reason.is_empty());
    assert_eq!(after.ms_total, 0);
}

#[test]
fn custom_sink_replaces_default_and_observes_every_event() {
    let mut mem = new_mem();
    let capture = Arc::new(CapturingSink::default());
    mem.set_event_sink(capture.clone() as SharedSink);

    // While a custom sink is installed, write_stats() must return None
    // — the default CountingSink fast path is disabled. Callers with
    // custom sinks read counters from their own handle.
    assert!(
        mem.write_stats().is_none(),
        "custom sink disables default write_stats()"
    );

    let _ = mem.store_raw("first real input", default_meta()).unwrap();
    let _ = mem.store_raw("   ", default_meta()).unwrap();
    let _ = mem.store_raw("second real input", default_meta()).unwrap();

    assert_eq!(capture.len(), 3, "every store_raw call records exactly one event");

    // Restoring the default sink works.
    mem.install_default_write_stats();
    let restored = mem.write_stats().expect("default sink back");
    assert_eq!(
        restored.total_calls(),
        0,
        "default sink has a fresh zeroed state after reinstall"
    );
    // Further writes should now flow into the default sink.
    let _ = mem.store_raw("after reinstall", default_meta()).unwrap();
    let restored2 = mem.write_stats().unwrap();
    assert_eq!(restored2.stored_count, 1);
}

#[test]
fn reset_write_stats_returns_false_when_custom_sink_installed() {
    let mut mem = new_mem();
    let capture = Arc::new(CapturingSink::default());
    mem.set_event_sink(capture as SharedSink);
    // No default sink to reset → must return false, never panic.
    let did_run = mem.reset_write_stats();
    assert!(!did_run);
}

#[test]
fn shared_counting_sink_from_user_code() {
    // Demonstrates the "plug your own CountingSink" pattern. Lets a
    // user keep their own Arc for sampling while Memory uses the same
    // sink for recording.
    let mut mem = new_mem();
    let counting = Arc::new(CountingSink::new());
    mem.set_event_sink(counting.clone() as SharedSink);

    let _ = mem.store_raw("hello world", default_meta()).unwrap();
    let _ = mem.store_raw("   ", default_meta()).unwrap();

    let s = counting.snapshot();
    assert_eq!(s.stored_count, 1);
    assert_eq!(s.skipped_count, 1);
    assert_eq!(s.total_calls(), 2);
}

#[test]
fn coverage_ratio_matches_pilot_formula() {
    // Sanity check on the coverage accessor: 3 stored + 1 skipped
    // should compute 0.75 exactly.
    let mut mem = new_mem();
    for i in 0..3 {
        let _ = mem
            .store_raw(&format!("row {i}"), default_meta())
            .unwrap();
    }
    let _ = mem.store_raw("   ", default_meta()).unwrap();
    let s = mem.write_stats().unwrap();
    assert_eq!(s.total_calls(), 4);
    assert_eq!(s.stored_count, 3);
    assert!(
        (s.coverage() - 0.75).abs() < 1e-9,
        "coverage = {}",
        s.coverage()
    );
}

#[test]
fn ms_total_grows_monotonically() {
    let mut mem = new_mem();
    let _ = mem.store_raw("first", default_meta()).unwrap();
    let t1 = mem.write_stats().unwrap().ms_total;

    // Wall-clock write — cheap but nonzero elapsed possible.
    let _ = mem.store_raw("second", default_meta()).unwrap();
    let t2 = mem.write_stats().unwrap().ms_total;

    // Monotonicity is the invariant the rebuild pilot actually cares
    // about; exact values are platform-dependent and may round to 0
    // for very fast writes.
    assert!(t2 >= t1, "ms_total must not decrease (t1={t1}, t2={t2})");
}
