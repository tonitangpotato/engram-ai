//! Write-path telemetry for `store_raw` / `store_enriched`.
//!
//! Root fix for Leak 2 (design.md §8): every call through the raw
//! write API emits a structured `StoreEvent`. Events flow through a
//! lightweight [`EventSink`] trait. The default implementation,
//! [`CountingSink`], accumulates in-memory counters exposed via
//! [`WriteStats`].
//!
//! ## Why a sink trait (and not just counters)
//!
//! The rebuild pilot (§9) needs to assert coverage thresholds
//! directly from stats — no SQL scraping. But different contexts
//! need different behavior:
//!
//! - Production: only counters, zero allocation per call.
//! - Tests: capture full event stream, inspect ordering.
//! - Future: push to metrics exporter (Prometheus / OTel).
//!
//! The `EventSink` trait lets each caller plug in what they need
//! without the hot path paying for the most-expensive option. All
//! sinks used by `Memory` must be `Send + Sync` so the struct stays
//! `Send`.
//!
//! ## Relationship to `log::info!`
//!
//! Before Step 8, `store_raw` emitted `info!` logs for skipped /
//! quarantined content. Those logs were useful to humans but
//! unstructured — a rebuild pilot couldn't aggregate them without
//! parsing text. Step 8 demotes those redundant `info!` calls to
//! `debug!`: the structured event is now the primary signal, and
//! `debug!` preserves the legacy text for humans tailing logs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::store_api::{ContentHash, MemoryId, QuarantineId, QuarantineReason, SkipReason};

/// Structured event emitted by the raw write path.
///
/// Every call to `Memory::store_raw` produces exactly one event,
/// even in the extractor-batch case (one `Stored` event carries the
/// fact count and aggregates the batch).
#[derive(Debug, Clone)]
pub enum StoreEvent {
    /// Content was successfully stored (inserted and/or merged).
    /// `fact_count` is the number of `StoreOutcome` entries produced
    /// by this single `store_raw` call — 1 for minimal-path, ≥1 for
    /// extractor-path where each fact becomes its own outcome.
    /// `merged_count` is how many of those outcomes were `Merged`.
    Stored {
        /// The id of the *first* outcome in the batch. Kept for
        /// correlation with external traces — callers that need the
        /// full list inspect the `RawStoreOutcome::Stored(Vec<..>)`
        /// return value directly.
        id: MemoryId,
        fact_count: usize,
        merged_count: usize,
        ms_elapsed: u64,
    },
    /// Content was intentionally not stored.
    Skipped {
        content_hash: ContentHash,
        reason: SkipReason,
        ms_elapsed: u64,
    },
    /// Extractor failed; content preserved in quarantine.
    Quarantined {
        id: QuarantineId,
        /// Kept full — `QuarantineReason` is cheap to clone (small
        /// strings at worst) and downstream sinks benefit from the
        /// typed variant rather than a flattened string.
        reason: QuarantineReason,
        ms_elapsed: u64,
    },
}

impl StoreEvent {
    /// Elapsed wall-clock time for the write call, in milliseconds.
    pub fn ms_elapsed(&self) -> u64 {
        match self {
            StoreEvent::Stored { ms_elapsed, .. }
            | StoreEvent::Skipped { ms_elapsed, .. }
            | StoreEvent::Quarantined { ms_elapsed, .. } => *ms_elapsed,
        }
    }
}

/// Convert an [`std::time::Duration`] to whole milliseconds,
/// saturating at `u64::MAX`. Used at every event-emission site so we
/// keep the wire type uniform.
pub fn duration_to_ms(d: Duration) -> u64 {
    // `as_millis()` returns u128; saturating cast protects against
    // the (impossible in practice) >580M-year elapsed window.
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Sink for write-path events.
///
/// Implementations must be cheap on the hot path — `record` is
/// called synchronously from inside `store_raw`. A slow sink becomes
/// write-path latency.
///
/// Send+Sync bound lets `Memory` hold an `Arc<dyn EventSink>` and
/// remain `Send`.
pub trait EventSink: Send + Sync {
    /// Record a single event. Must not panic.
    fn record(&self, event: StoreEvent);
}

/// Snapshot of accumulated write-path counters.
///
/// All counts are monotonic since the last [`Memory::reset_write_stats`]
/// (or since process start). `skipped_by_reason` sums over all skip
/// reasons; the total equals `skipped_count`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteStats {
    /// Number of `store_raw` calls that produced at least one stored row.
    pub stored_count: u64,
    /// Sum of fact counts across all `Stored` events — i.e., total
    /// rows written or merged. Always ≥ `stored_count`.
    pub stored_fact_count: u64,
    /// Number of `Merged` outcomes (subset of `stored_fact_count`).
    pub merged_count: u64,
    /// Number of `store_raw` calls that were intentionally skipped.
    pub skipped_count: u64,
    /// Number of `store_raw` calls that landed in quarantine.
    pub quarantined_count: u64,
    /// Skip reason histogram. Sum of values == `skipped_count`.
    pub skipped_by_reason: HashMap<SkipReason, u64>,
    /// Total elapsed time across all recorded events, in ms. Useful
    /// for coarse throughput estimates (`total_calls / ms_total`).
    pub ms_total: u64,
}

impl WriteStats {
    /// Total number of `store_raw` calls recorded.
    ///
    /// Equals `stored_count + skipped_count + quarantined_count`.
    /// The rebuild pilot's "coverage" assertion (§9) is
    /// `stored_count as f64 / total_calls() as f64 > 0.95`.
    pub fn total_calls(&self) -> u64 {
        self.stored_count + self.skipped_count + self.quarantined_count
    }

    /// Coverage ratio — fraction of calls that produced stored rows.
    ///
    /// Returns 0.0 if no calls have been recorded. The pilot asserts
    /// `> 0.95` on a representative input set.
    pub fn coverage(&self) -> f64 {
        let total = self.total_calls();
        if total == 0 {
            0.0
        } else {
            self.stored_count as f64 / total as f64
        }
    }
}

/// Default [`EventSink`] — accumulates counters in memory.
///
/// Thread-safe via `Mutex<WriteStats>`. The mutex is only held for
/// the increment; no sink call ever blocks on IO.
#[derive(Debug, Default)]
pub struct CountingSink {
    inner: Mutex<WriteStats>,
}

impl CountingSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot the current counters. Cheap — clones a few u64s and
    /// a small HashMap. Callers are expected to snapshot infrequently
    /// (per pilot report, per heartbeat), not per write.
    pub fn snapshot(&self) -> WriteStats {
        self.inner.lock().unwrap().clone()
    }

    /// Zero all counters. Used by tests and by pilot code that
    /// wants a clean window around a batch.
    pub fn reset(&self) {
        *self.inner.lock().unwrap() = WriteStats::default();
    }
}

impl EventSink for CountingSink {
    fn record(&self, event: StoreEvent) {
        let mut stats = self.inner.lock().unwrap();
        stats.ms_total = stats.ms_total.saturating_add(event.ms_elapsed());
        match event {
            StoreEvent::Stored {
                fact_count,
                merged_count,
                ..
            } => {
                stats.stored_count = stats.stored_count.saturating_add(1);
                stats.stored_fact_count = stats
                    .stored_fact_count
                    .saturating_add(fact_count as u64);
                stats.merged_count = stats
                    .merged_count
                    .saturating_add(merged_count as u64);
            }
            StoreEvent::Skipped { reason, .. } => {
                stats.skipped_count = stats.skipped_count.saturating_add(1);
                *stats.skipped_by_reason.entry(reason).or_insert(0) += 1;
            }
            StoreEvent::Quarantined { .. } => {
                stats.quarantined_count =
                    stats.quarantined_count.saturating_add(1);
            }
        }
    }
}

/// Convenience alias — the shared-reference type `Memory` holds.
///
/// `Arc<dyn EventSink>` keeps the sink cheap to clone (so tests can
/// keep one reference while `Memory` holds another) and erases the
/// concrete type (so swapping `CountingSink` for a metrics exporter
/// is a one-line change in the caller).
pub type SharedSink = Arc<dyn EventSink>;

/// No-op sink. Used as the placeholder when a caller explicitly
/// wants stats disabled. Kept for tests and for future "stats
/// off" config; `Memory` defaults to `CountingSink` so production
/// gets counters for free.
#[derive(Debug, Default)]
pub struct NoopSink;

impl EventSink for NoopSink {
    fn record(&self, _event: StoreEvent) {}
}

// --------------------------------------------------------------------
// Unit tests
// --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store_api::{ContentHash, QuarantineId, QuarantineReason, SkipReason};

    fn mk_stored(n: usize, merged: usize, ms: u64) -> StoreEvent {
        StoreEvent::Stored {
            id: "m-test".to_string(),
            fact_count: n,
            merged_count: merged,
            ms_elapsed: ms,
        }
    }

    fn mk_skipped(reason: SkipReason, ms: u64) -> StoreEvent {
        StoreEvent::Skipped {
            content_hash: ContentHash::new("h-test".to_string()),
            reason,
            ms_elapsed: ms,
        }
    }

    fn mk_quarantined(reason: QuarantineReason, ms: u64) -> StoreEvent {
        StoreEvent::Quarantined {
            id: QuarantineId::new("q-test".to_string()),
            reason,
            ms_elapsed: ms,
        }
    }

    #[test]
    fn counting_sink_accumulates_stored() {
        let sink = CountingSink::new();
        sink.record(mk_stored(1, 0, 10));
        sink.record(mk_stored(3, 1, 20));
        let s = sink.snapshot();
        assert_eq!(s.stored_count, 2);
        assert_eq!(s.stored_fact_count, 4);
        assert_eq!(s.merged_count, 1);
        assert_eq!(s.skipped_count, 0);
        assert_eq!(s.quarantined_count, 0);
        assert_eq!(s.ms_total, 30);
    }

    #[test]
    fn counting_sink_buckets_skip_reasons() {
        let sink = CountingSink::new();
        sink.record(mk_skipped(SkipReason::TooShort, 1));
        sink.record(mk_skipped(SkipReason::TooShort, 1));
        sink.record(mk_skipped(SkipReason::NoFactsExtracted, 2));
        sink.record(mk_skipped(SkipReason::DuplicateContent, 3));
        let s = sink.snapshot();
        assert_eq!(s.skipped_count, 4);
        assert_eq!(s.skipped_by_reason.get(&SkipReason::TooShort), Some(&2));
        assert_eq!(
            s.skipped_by_reason.get(&SkipReason::NoFactsExtracted),
            Some(&1)
        );
        assert_eq!(
            s.skipped_by_reason.get(&SkipReason::DuplicateContent),
            Some(&1)
        );
        let histogram_total: u64 = s.skipped_by_reason.values().sum();
        assert_eq!(histogram_total, s.skipped_count);
    }

    #[test]
    fn counting_sink_tracks_quarantine() {
        let sink = CountingSink::new();
        sink.record(mk_quarantined(QuarantineReason::ExtractorPanic, 5));
        sink.record(mk_quarantined(
            QuarantineReason::ExtractorError("503".to_string()),
            10,
        ));
        let s = sink.snapshot();
        assert_eq!(s.quarantined_count, 2);
        assert_eq!(s.ms_total, 15);
    }

    #[test]
    fn total_calls_is_sum_of_three_buckets() {
        let sink = CountingSink::new();
        sink.record(mk_stored(1, 0, 1));
        sink.record(mk_skipped(SkipReason::TooShort, 1));
        sink.record(mk_skipped(SkipReason::NoFactsExtracted, 1));
        sink.record(mk_quarantined(QuarantineReason::ExtractorPanic, 1));
        let s = sink.snapshot();
        assert_eq!(s.total_calls(), 4);
        assert_eq!(s.stored_count, 1);
        assert_eq!(s.skipped_count, 2);
        assert_eq!(s.quarantined_count, 1);
    }

    #[test]
    fn coverage_matches_pilot_formula() {
        let sink = CountingSink::new();
        for _ in 0..96 {
            sink.record(mk_stored(1, 0, 1));
        }
        for _ in 0..3 {
            sink.record(mk_skipped(SkipReason::NoFactsExtracted, 1));
        }
        sink.record(mk_quarantined(QuarantineReason::ExtractorPanic, 1));
        let s = sink.snapshot();
        assert_eq!(s.total_calls(), 100);
        assert!(
            s.coverage() > 0.95,
            "coverage = {} (pilot §9 threshold 0.95)",
            s.coverage()
        );
    }

    #[test]
    fn coverage_returns_zero_on_empty() {
        let sink = CountingSink::new();
        let s = sink.snapshot();
        assert_eq!(s.total_calls(), 0);
        assert_eq!(s.coverage(), 0.0);
    }

    #[test]
    fn reset_zeroes_all_counters() {
        let sink = CountingSink::new();
        sink.record(mk_stored(2, 1, 5));
        sink.record(mk_skipped(SkipReason::TooShort, 1));
        sink.record(mk_quarantined(QuarantineReason::ExtractorPanic, 2));
        sink.reset();
        let s = sink.snapshot();
        assert_eq!(s, WriteStats::default());
        assert_eq!(s.total_calls(), 0);
        assert!(s.skipped_by_reason.is_empty());
    }

    #[test]
    fn noop_sink_records_nothing() {
        let sink = NoopSink;
        // Should not panic, should not do anything observable.
        sink.record(mk_stored(1, 0, 1));
        sink.record(mk_skipped(SkipReason::TooShort, 1));
        sink.record(mk_quarantined(QuarantineReason::ExtractorPanic, 1));
    }

    #[test]
    fn shared_sink_via_arc_observes_all_events() {
        // Reconstructs the production wiring: Memory holds
        // Arc<dyn EventSink>, test holds another Arc to the same
        // counter, both see the same writes.
        let counting = Arc::new(CountingSink::new());
        let sink: SharedSink = counting.clone();
        sink.record(mk_stored(1, 0, 1));
        sink.record(mk_skipped(SkipReason::DuplicateContent, 1));
        let s = counting.snapshot();
        assert_eq!(s.stored_count, 1);
        assert_eq!(s.skipped_count, 1);
    }

    #[test]
    fn duration_to_ms_saturates() {
        assert_eq!(duration_to_ms(Duration::from_millis(0)), 0);
        assert_eq!(duration_to_ms(Duration::from_millis(42)), 42);
        // Very large but finite — should still map cleanly.
        assert_eq!(duration_to_ms(Duration::from_secs(60)), 60_000);
    }

    #[test]
    fn store_event_ms_elapsed_accessor() {
        assert_eq!(mk_stored(1, 0, 7).ms_elapsed(), 7);
        assert_eq!(mk_skipped(SkipReason::TooShort, 11).ms_elapsed(), 11);
        assert_eq!(
            mk_quarantined(QuarantineReason::ExtractorPanic, 13).ms_elapsed(),
            13
        );
    }

    #[test]
    fn histogram_sum_invariant_holds_under_mixed_load() {
        // Property-style: sum of skipped_by_reason values must
        // always equal skipped_count, even under mixed/interleaved
        // events. Catches off-by-one in the increment logic.
        let sink = CountingSink::new();
        let reasons = [
            SkipReason::TooShort,
            SkipReason::NoFactsExtracted,
            SkipReason::DuplicateContent,
        ];
        for i in 0..50u32 {
            let r = reasons[(i as usize) % reasons.len()];
            sink.record(mk_skipped(r, 1));
            if i % 5 == 0 {
                sink.record(mk_stored(1, 0, 1));
            }
            if i % 7 == 0 {
                sink.record(mk_quarantined(QuarantineReason::ExtractorPanic, 1));
            }
        }
        let s = sink.snapshot();
        let histogram_total: u64 = s.skipped_by_reason.values().sum();
        assert_eq!(histogram_total, s.skipped_count);
    }
}
