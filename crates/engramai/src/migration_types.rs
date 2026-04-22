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

/// Minimum content length for `NeedsBackfill` classification — rows
/// shorter than this that are missing dimensions are allowed to stay
/// minimal (short content rarely yields meaningful re-extraction).
///
/// See design §6 ("NeedsBackfill" criteria).
pub const BACKFILL_MIN_CONTENT_LEN: usize = 40;

/// Classify a stored metadata blob against the v2 (namespaced) / v1
/// (flat) / pre-extractor layouts.
///
/// Pure function — no storage, no side effects. Callers decide whether
/// to act on the classification (enqueue for backfill, rewrite as v2,
/// etc.). See design §6 ("Migration strategy").
///
/// Returns `None` if the metadata is already v2 (no action needed).
/// Returns `Some(classification)` otherwise.
///
/// Classification rules (design §6):
///
/// - **Already v2** (`engram.dimensions` present) → `None`
/// - **HasExtractorData**: v1 row has a `dimensions` object with at
///   least one of {participants, temporal, causation} AND at least
///   one scalar (domain or valence). Lossless upgrade.
/// - **LowDimLegacy{MissingCoreDimensions}**: v1 row has a `dimensions`
///   object but it lacks all of {participants, temporal, causation}.
///   Long content → re-extraction likely helps.
/// - **LowDimLegacy{DimensionsEmpty}**: v1 row has an empty
///   `dimensions: {}`.
/// - **LowDimLegacy{PartialDimensionsLongContent}**: v1 row has some
///   narrative fields but missing ≥1 core dimension AND content is
///   long (≥ `BACKFILL_MIN_CONTENT_LEN`).
/// - **UnparseableLegacy**: metadata is not a JSON object at all, or
///   there is no `dimensions` field (pre-extractor era). Short content
///   stays minimal (no backfill); long content is `PreExtraction`-style
///   legacy and goes to the queue.
pub fn classify_stored_metadata(
    metadata: &serde_json::Value,
    content_len: usize,
) -> Option<LegacyClassification> {
    // v2 — nothing to do.
    if metadata
        .get("engram")
        .and_then(|e| e.get("dimensions"))
        .is_some()
    {
        return None;
    }

    // Must be a JSON object to inspect further. Non-object → unparseable.
    let obj = match metadata.as_object() {
        Some(o) => o,
        None => {
            return Some(LegacyClassification::UnparseableLegacy {
                error: "metadata is not a JSON object".to_string(),
            });
        }
    };

    // v1 row structure: look for `dimensions` field (object) at top level.
    let dims_obj = obj.get("dimensions").and_then(|v| v.as_object());

    match dims_obj {
        // Pre-extractor: no `dimensions` field at all.
        None => {
            // Short content → not worth queueing; caller treats as
            // minimal Dimensions. We still return UnparseableLegacy so
            // callers that want to log a warning have a signal, but
            // they can check `content_len` before enqueuing.
            if content_len < BACKFILL_MIN_CONTENT_LEN {
                Some(LegacyClassification::UnparseableLegacy {
                    error: "no dimensions field; content too short for backfill".to_string(),
                })
            } else {
                Some(LegacyClassification::LowDimLegacy {
                    reason: BackfillReason::MissingCoreDimensions,
                })
            }
        }
        Some(dims) if dims.is_empty() => {
            if content_len < BACKFILL_MIN_CONTENT_LEN {
                Some(LegacyClassification::UnparseableLegacy {
                    error: "dimensions empty; content too short for backfill".to_string(),
                })
            } else {
                Some(LegacyClassification::LowDimLegacy {
                    reason: BackfillReason::DimensionsEmpty,
                })
            }
        }
        Some(dims) => {
            let has_participants = dims
                .get("participants")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let has_temporal = dims
                .get("temporal")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let has_causation = dims
                .get("causation")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let has_domain = dims
                .get("domain")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let has_valence = dims.get("valence").and_then(|v| v.as_f64()).is_some();

            let has_any_core = has_participants || has_temporal || has_causation;
            let has_any_scalar = has_domain || has_valence;

            if has_any_core && has_any_scalar {
                Some(LegacyClassification::HasExtractorData)
            } else if !has_any_core {
                // Has the `dimensions` scaffold but no core narrative fields.
                if content_len < BACKFILL_MIN_CONTENT_LEN {
                    Some(LegacyClassification::UnparseableLegacy {
                        error: "no core dimensions; content too short for backfill"
                            .to_string(),
                    })
                } else {
                    Some(LegacyClassification::LowDimLegacy {
                        reason: BackfillReason::MissingCoreDimensions,
                    })
                }
            } else {
                // Has some narrative core fields but lacks scalars → partial.
                if content_len < BACKFILL_MIN_CONTENT_LEN {
                    // Still HasExtractorData — partial is fine for short content.
                    Some(LegacyClassification::HasExtractorData)
                } else {
                    Some(LegacyClassification::LowDimLegacy {
                        reason: BackfillReason::PartialDimensionsLongContent,
                    })
                }
            }
        }
    }
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

/// Summary of a `scan_and_enqueue_backfill` pass.
///
/// Reports how many `memories` rows were inspected and categorised
/// by the v1 → classification dispatch. Only `LowDimLegacy` rows
/// are enqueued for re-extraction; everything else is surfaced for
/// observability.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReport {
    /// Rows inspected (possibly across multiple SQL pages).
    pub scanned: u64,
    /// Rows that already had v2 metadata (no-op, not enqueued).
    pub already_v2: u64,
    /// v1 rows with enough dimensional data to upgrade losslessly on
    /// next write (not enqueued — re-extraction wouldn't help).
    pub has_extractor_data: u64,
    /// Rows newly added to `backfill_queue` this scan.
    pub enqueued: u64,
    /// v1 rows skipped because content was too short or metadata was
    /// unparseable (not worth re-extracting).
    pub unparseable: u64,
}

impl ScanReport {
    pub fn total(&self) -> u64 {
        self.scanned
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

    // ---- classify_stored_metadata ----

    #[test]
    fn classify_v2_returns_none() {
        let meta = serde_json::json!({
            "engram": {
                "version": 2,
                "dimensions": {"participants": "alice"}
            }
        });
        assert_eq!(classify_stored_metadata(&meta, 100), None);
    }

    #[test]
    fn classify_v1_with_full_dimensions_is_has_extractor_data() {
        let meta = serde_json::json!({
            "dimensions": {
                "participants": "alice",
                "domain": "coding",
                "valence": 0.2
            }
        });
        assert_eq!(
            classify_stored_metadata(&meta, 200),
            Some(LegacyClassification::HasExtractorData)
        );
    }

    #[test]
    fn classify_v1_empty_dimensions_long_content_is_dimensions_empty() {
        let meta = serde_json::json!({ "dimensions": {} });
        assert_eq!(
            classify_stored_metadata(&meta, 200),
            Some(LegacyClassification::LowDimLegacy {
                reason: BackfillReason::DimensionsEmpty
            })
        );
    }

    #[test]
    fn classify_v1_empty_dimensions_short_content_is_unparseable() {
        let meta = serde_json::json!({ "dimensions": {} });
        let got = classify_stored_metadata(&meta, 10);
        match got {
            Some(LegacyClassification::UnparseableLegacy { .. }) => (),
            other => panic!("expected UnparseableLegacy, got {:?}", other),
        }
    }

    #[test]
    fn classify_v1_no_dimensions_field_long_is_missing_core() {
        // Pre-extractor era: no `dimensions` field at all, long content.
        let meta = serde_json::json!({ "merge_count": 0 });
        assert_eq!(
            classify_stored_metadata(&meta, 200),
            Some(LegacyClassification::LowDimLegacy {
                reason: BackfillReason::MissingCoreDimensions
            })
        );
    }

    #[test]
    fn classify_v1_no_dimensions_field_short_is_unparseable() {
        let meta = serde_json::json!({});
        let got = classify_stored_metadata(&meta, 10);
        match got {
            Some(LegacyClassification::UnparseableLegacy { .. }) => (),
            other => panic!("expected UnparseableLegacy, got {:?}", other),
        }
    }

    #[test]
    fn classify_v1_dimensions_without_core_narrative_is_missing_core() {
        // Has `dimensions` with scalars only — no participants/temporal/causation.
        let meta = serde_json::json!({
            "dimensions": {
                "domain": "coding",
                "valence": 0.1
            }
        });
        assert_eq!(
            classify_stored_metadata(&meta, 200),
            Some(LegacyClassification::LowDimLegacy {
                reason: BackfillReason::MissingCoreDimensions
            })
        );
    }

    #[test]
    fn classify_v1_partial_long_content_is_partial_long() {
        // Has core narrative but missing scalars, long content.
        let meta = serde_json::json!({
            "dimensions": {
                "participants": "alice"
            }
        });
        assert_eq!(
            classify_stored_metadata(&meta, 200),
            Some(LegacyClassification::LowDimLegacy {
                reason: BackfillReason::PartialDimensionsLongContent
            })
        );
    }

    #[test]
    fn classify_v1_partial_short_content_is_has_extractor_data() {
        // Short content with partial dimensions is allowed to stay minimal.
        let meta = serde_json::json!({
            "dimensions": {
                "participants": "alice"
            }
        });
        assert_eq!(
            classify_stored_metadata(&meta, 10),
            Some(LegacyClassification::HasExtractorData)
        );
    }

    #[test]
    fn classify_non_object_is_unparseable() {
        let meta = serde_json::json!(42);
        match classify_stored_metadata(&meta, 100) {
            Some(LegacyClassification::UnparseableLegacy { .. }) => (),
            other => panic!("expected UnparseableLegacy, got {:?}", other),
        }
    }

    #[test]
    fn classify_whitespace_only_fields_dont_count() {
        let meta = serde_json::json!({
            "dimensions": {
                "participants": "   ",
                "temporal": "",
                "causation": "\t"
            }
        });
        // All core fields whitespace-only → treated as missing core.
        assert_eq!(
            classify_stored_metadata(&meta, 200),
            Some(LegacyClassification::LowDimLegacy {
                reason: BackfillReason::MissingCoreDimensions
            })
        );
    }
}
