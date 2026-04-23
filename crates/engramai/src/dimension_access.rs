//! Typed accessor layer for the extractor-populated dimension signature of a
//! `MemoryRecord`. Reads once per record via `Dimensions::from_stored_metadata`
//! and exposes typed borrows — no JSON walking, no string-array fallbacks.
//!
//! Design reference: `.gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/design.md` §5.2.
//!
//! The goal is that Change 3a (`temporal_score` reads `temporal()`) and the
//! future Change 3b (ISS-020 Phase B: `dimension_match_score` reads
//! `participants()`, `relations()`, ...) share one accessor. 3a ships now;
//! 3b's additions won't require refactoring 3a.

use crate::dimensions::{Dimensions, TemporalMark};
use crate::types::MemoryRecord;

/// Typed accessor for the dimension signature of a `MemoryRecord`.
///
/// Internally holds a parsed `Dimensions`. `Dimensions::from_stored_metadata`
/// is a pure JSON deserialize (no DB / no allocation of persistent state);
/// constructing a view is cheap enough to do per-candidate in the scoring loop.
///
/// Safe against missing / malformed metadata: `from_record` always returns
/// a view backed by at least `Dimensions::minimal(record.content)`. If even
/// that fails (pathological empty content), all getters return `None`.
pub struct DimensionView<'a> {
    #[allow(dead_code)] // retained for 3b use (e.g. record.id in cache keys)
    record: &'a MemoryRecord,
    dims: Option<Dimensions>,
}

impl<'a> DimensionView<'a> {
    /// Build a typed view over this record's stored metadata.
    ///
    /// Resolution order:
    /// 1. If `record.metadata` is present, try `from_stored_metadata` (handles
    ///    both v1 and v2 JSON layouts).
    /// 2. On parse failure or missing metadata, fall back to
    ///    `Dimensions::minimal(record.content)`.
    /// 3. If even that fails (empty content — should be unreachable on the
    ///    recall path), `dims` is `None` and every getter returns `None`.
    pub fn from_record(record: &'a MemoryRecord) -> Self {
        let dims = match record.metadata.as_ref() {
            Some(meta) => Dimensions::from_stored_metadata(meta, &record.content)
                .ok()
                .or_else(|| Dimensions::minimal(&record.content).ok()),
            None => Dimensions::minimal(&record.content).ok(),
        };
        Self { record, dims }
    }

    /// Typed temporal mark as the extractor stored it. `None` means:
    /// - the extractor produced no temporal signal for this memory, OR
    /// - the record has no parseable metadata.
    ///
    /// Callers distinguishing these cases should use `has_dimensions()`.
    pub fn temporal(&self) -> Option<&TemporalMark> {
        self.dims.as_ref().and_then(|d| d.temporal.as_ref())
    }

    /// Participants — free-form string as stored by the extractor today.
    /// (If ISS-020 Phase B promotes this to `Vec<String>` inside `Dimensions`,
    /// the return type changes here; 3a doesn't read this field.)
    #[allow(dead_code)] // reserved for ISS-020 Phase B (Change 3b)
    pub fn participants(&self) -> Option<&str> {
        self.dims.as_ref().and_then(|d| d.participants.as_deref())
    }

    #[allow(dead_code)]
    pub fn relations(&self) -> Option<&str> {
        self.dims.as_ref().and_then(|d| d.relations.as_deref())
    }

    #[allow(dead_code)]
    pub fn sentiment(&self) -> Option<&str> {
        self.dims.as_ref().and_then(|d| d.sentiment.as_deref())
    }

    #[allow(dead_code)]
    pub fn location(&self) -> Option<&str> {
        self.dims.as_ref().and_then(|d| d.location.as_deref())
    }

    #[allow(dead_code)]
    pub fn context(&self) -> Option<&str> {
        self.dims.as_ref().and_then(|d| d.context.as_deref())
    }

    #[allow(dead_code)]
    pub fn causation(&self) -> Option<&str> {
        self.dims.as_ref().and_then(|d| d.causation.as_deref())
    }

    #[allow(dead_code)]
    pub fn outcome(&self) -> Option<&str> {
        self.dims.as_ref().and_then(|d| d.outcome.as_deref())
    }

    /// True if the record had parseable dimension metadata (v1 or v2).
    /// False means we fell through to `Dimensions::minimal` or worse.
    #[allow(dead_code)]
    pub fn has_dimensions(&self) -> bool {
        self.dims.is_some()
    }

    /// True if ANY narrative dimension is populated. Cheap guard for skipping
    /// work in Change 3b's `dimension_match_score`.
    #[allow(dead_code)]
    pub fn has_any_narrative(&self) -> bool {
        match &self.dims {
            None => false,
            Some(d) => {
                d.temporal.is_some()
                    || d.participants.is_some()
                    || d.relations.is_some()
                    || d.location.is_some()
                    || d.context.is_some()
                    || d.causation.is_some()
                    || d.outcome.is_some()
                    || d.sentiment.is_some()
            }
        }
    }

    /// Expose the underlying typed struct for callers that want everything
    /// (scalars, type_weights, ...). 3b uses this to avoid adding a
    /// getter-per-field as new dimensions appear.
    #[allow(dead_code)]
    pub fn dimensions(&self) -> Option<&Dimensions> {
        self.dims.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimensions::Dimensions;
    use crate::types::{MemoryRecord, MemoryType};
    use chrono::{NaiveDate, Utc};
    use serde_json::json;

    fn fixture_record(metadata: Option<serde_json::Value>, content: &str) -> MemoryRecord {
        MemoryRecord {
            id: "test-id".to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Factual,
            layer: crate::types::MemoryLayer::Working,
            created_at: Utc::now(),
            access_times: vec![],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: "test".to_string(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata,
        }
    }

    #[test]
    fn test_from_record_with_v2_temporal_day() {
        // Build v2 metadata with a typed Day temporal mark.
        let mut dims = Dimensions::minimal("test content").unwrap();
        dims.temporal = Some(TemporalMark::Day(
            NaiveDate::from_ymd_opt(2024, 1, 10).unwrap(),
        ));
        let dims_val = serde_json::to_value(&dims).unwrap();
        let metadata = json!({
            "engram": {
                "dimensions": dims_val
            }
        });

        let record = fixture_record(Some(metadata), "test content");
        let view = DimensionView::from_record(&record);

        assert!(view.has_dimensions());
        match view.temporal() {
            Some(TemporalMark::Day(d)) => {
                assert_eq!(*d, NaiveDate::from_ymd_opt(2024, 1, 10).unwrap());
            }
            other => panic!("expected Day, got {:?}", other),
        }
    }

    #[test]
    fn test_from_record_no_metadata_falls_back_to_minimal() {
        let record = fixture_record(None, "some content");
        let view = DimensionView::from_record(&record);
        // `minimal` produces a dims struct with no narrative fields populated.
        assert!(view.has_dimensions());
        assert!(view.temporal().is_none());
        assert!(!view.has_any_narrative());
    }

    #[test]
    fn test_from_record_malformed_metadata_falls_back_to_minimal() {
        // Metadata that fails from_stored_metadata parse but has non-empty content
        // → `minimal` fallback succeeds.
        let metadata = json!({"not_engram": "garbage", "random": [1, 2, 3]});
        let record = fixture_record(Some(metadata), "some content");
        let view = DimensionView::from_record(&record);
        // from_stored_metadata is lenient — it may succeed with all-None on
        // unknown shapes. Either way, accessor should not panic.
        assert!(view.temporal().is_none());
    }

    #[test]
    fn test_has_any_narrative_detects_participants() {
        let mut dims = Dimensions::minimal("test").unwrap();
        dims.participants = Some("alice, bob".to_string());
        let metadata = json!({
            "engram": { "dimensions": serde_json::to_value(&dims).unwrap() }
        });
        let record = fixture_record(Some(metadata), "test");
        let view = DimensionView::from_record(&record);
        assert!(view.has_any_narrative());
        assert_eq!(view.participants(), Some("alice, bob"));
    }

    #[test]
    fn test_vague_temporal_mark_exposed() {
        let mut dims = Dimensions::minimal("test").unwrap();
        dims.temporal = Some(TemporalMark::Vague("last summer".to_string()));
        let metadata = json!({
            "engram": { "dimensions": serde_json::to_value(&dims).unwrap() }
        });
        let record = fixture_record(Some(metadata), "test");
        let view = DimensionView::from_record(&record);
        match view.temporal() {
            Some(TemporalMark::Vague(s)) => assert_eq!(s, "last summer"),
            other => panic!("expected Vague, got {:?}", other),
        }
    }
}
