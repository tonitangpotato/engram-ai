//! Migration / telemetry types.
//!
//! Supports Step 7 (v1 legacy read path + classification) and Step 8
//! (WriteStats telemetry + backfill_dimensions API).
//!
//! See design §6, §8 of ISS-019.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::store_api::SkipReason;

// ---------------------------------------------------------------------
// Legacy classification
// ---------------------------------------------------------------------

/// How a v1 (pre-ISS-019) metadata row should be handled when read.
///
/// Produced by `Dimensions::from_stored_metadata` (Step 7) when it
/// encounters a row missing the `engram.dimensions` namespaced layout.
///
/// - `HasExtractorData`: old-format row still has enough dimensional
///   information to upgrade losslessly.
/// - `LowDimLegacy`: row is missing the key dimensions and content is
///   long enough that re-extraction would likely recover more. Enqueued
///   into `backfill_queue`.
/// - `UnparseableLegacy`: corrupt JSON / missing mandatory fields —
///   unfixable, served as minimal Dimensions, not enqueued.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum LegacyClassification {
    HasExtractorData,
    LowDimLegacy { reason: BackfillReason },
    UnparseableLegacy { error: String },
}

/// Why a v1 row was enqueued for dimensional backfill.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillReason {
    /// Row missing participants, temporal, and causation — extractor
    /// was likely absent at original write time.
    MissingCoreDimensions,
    /// Row had a `dimensions` blob but it was empty/skeletal.
    DimensionsEmpty,
    /// Row had partial dimensions but content length > threshold and
    /// extractor is now available.
    PartialDimensionsLongContent,
}

// ---------------------------------------------------------------------
// Backfill report
// ---------------------------------------------------------------------

/// Summary of a `backfill_dimensions` pass.
///
/// Returned by the Step 8 backfill API; aggregated across multiple
/// passes in the Step 10 full-rebuild driver.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BackfillReport {
    /// Rows attempted (matched `backfill_queue` and still v1).
    pub attempted: u64,
    /// Rows successfully re-extracted and rewritten as v2.
    pub upgraded: u64,
    /// Rows inspected but found already-v2 (no-op; cleaned from queue).
    pub unchanged: u64,
    /// Rows where re-extraction failed; stay in queue for next pass.
    pub failed: u64,
    /// Rows permanently rejected (attempts >= max).
    pub permanently_rejected: u64,
}

impl BackfillReport {
    pub fn total(&self) -> u64 {
        self.attempted
    }

    pub fn success_rate(&self) -> f64 {
        if self.attempted == 0 {
            return 0.0;
        }
        self.upgraded as f64 / self.attempted as f64
    }
}

// ---------------------------------------------------------------------
// Write-path telemetry
// ---------------------------------------------------------------------

/// Counters maintained by the write pipeline for observability.
///
/// Mirrors the taxonomy of `RawStoreOutcome` plus merge tracking.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WriteStats {
    pub stored: u64,
    pub merged: u64,
    pub skipped: u64,
    pub quarantined: u64,
    pub skipped_by_reason: HashMap<SkipReason, u64>,
}

impl WriteStats {
    pub fn total(&self) -> u64 {
        self.stored + self.merged + self.skipped + self.quarantined
    }
}

/// Event hook emitted for each write decision (for logs / tracing).
///
/// Consumers subscribe via the existing empathy bus (or a dedicated
/// store-event channel, TBD Step 8). Variants intentionally carry
/// small payloads — detailed data stays on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum StoreEvent {
    Stored { id: String },
    Merged { id: String, similarity: f32 },
    Skipped { reason: SkipReason },
    Quarantined { id: String },
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_classification_serde_roundtrip() {
        for c in [
            LegacyClassification::HasExtractorData,
            LegacyClassification::LowDimLegacy {
                reason: BackfillReason::MissingCoreDimensions,
            },
            LegacyClassification::UnparseableLegacy {
                error: "bad json".to_string(),
            },
        ] {
            let j = serde_json::to_string(&c).unwrap();
            let back: LegacyClassification = serde_json::from_str(&j).unwrap();
            assert_eq!(c, back);
        }
    }

    #[test]
    fn backfill_report_defaults() {
        let r = BackfillReport::default();
        assert_eq!(r.attempted, 0);
        assert_eq!(r.success_rate(), 0.0);
    }

    #[test]
    fn backfill_report_success_rate() {
        let r = BackfillReport {
            attempted: 10,
            upgraded: 7,
            unchanged: 1,
            failed: 2,
            permanently_rejected: 0,
        };
        assert!((r.success_rate() - 0.7).abs() < 1e-9);
    }

    #[test]
    fn write_stats_total() {
        let s = WriteStats {
            stored: 10,
            merged: 2,
            skipped: 1,
            quarantined: 0,
            skipped_by_reason: HashMap::new(),
        };
        assert_eq!(s.total(), 13);
    }

    #[test]
    fn store_event_serde_roundtrip() {
        for ev in [
            StoreEvent::Stored {
                id: "m1".to_string(),
            },
            StoreEvent::Merged {
                id: "m2".to_string(),
                similarity: 0.88,
            },
            StoreEvent::Skipped {
                reason: SkipReason::NoFactsExtracted,
            },
            StoreEvent::Quarantined {
                id: "q1".to_string(),
            },
        ] {
            let j = serde_json::to_string(&ev).unwrap();
            let back: StoreEvent = serde_json::from_str(&j).unwrap();
            assert_eq!(ev, back);
        }
    }

    #[test]
    fn backfill_reason_hashmap_key() {
        // Sanity: BackfillReason derives Eq + Hash, so it can key a map.
        let mut m: HashMap<BackfillReason, u64> = HashMap::new();
        m.insert(BackfillReason::MissingCoreDimensions, 5);
        assert_eq!(m.get(&BackfillReason::MissingCoreDimensions), Some(&5));
    }
}
