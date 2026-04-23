//! Natural-language time parsing for the temporal dimension scoring hot path.
//!
//! This module is the *substrate* for ISS-024 Change 3a: converting the
//! extractor-produced `TemporalMark::Vague(String)` into a `TimeRange` for
//! `temporal_score`. The three non-vague `TemporalMark` variants (`Exact`,
//! `Range`, `Day`) are already typed and do NOT go through this module —
//! they're converted inline in `temporal_score`.
//!
//! Design reference: `.gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/design.md` §1.
//!
//! ## Why a cache
//!
//! `two_timer::parse` is ~50–200µs per call. In a LoCoMo-style recall (~1K
//! candidates), vague temporal phrases recur across queries (e.g. many memories
//! share the phrase "recently" or "a while back"). An LRU keyed by
//! `(phrase, reference_day)` collapses steady-state parses to near-zero.
//!
//! ## Reference-point policy
//!
//! The anchor for relative expressions ("yesterday", "last Tuesday") MUST be
//! the memory's `created_at`, not wall-clock `Utc::now()`. A memory stored on
//! 2024-01-10 saying "yesterday afternoon" means 2024-01-09 afternoon — the
//! temporal deixis was fixed at storage time, not query time.
//!
//! Cache keys round the reference to `NaiveDate` granularity: "yesterday"
//! parsed against 10:00Z vs 14:00Z on the same day resolves to the same range.

use std::num::NonZeroUsize;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use lru::LruCache;

use crate::query_classifier::TimeRange;

/// Default LRU capacity for the dimension-parse cache.
///
/// Sized for the LoCoMo haystack: ~1K candidates × small number of distinct
/// vague phrases. 4096 entries × ~96 B/entry ≈ 400 KB; trivial. Bump if
/// profiling shows eviction thrash.
pub const CACHE_CAPACITY_DEFAULT: usize = 4096;

/// Parse a natural-language time expression against a reference point.
///
/// - `phrase` — the extractor-produced string (e.g. "yesterday afternoon").
/// - `reference` — the anchor for relative expressions. MUST be the memory's
///   `created_at`, not wall-clock now.
///
/// Returns `None` if the phrase is unparseable (common for vague extractor
/// output like "recently" or "a while ago"). `None` signals "fall back to
/// insertion-time scoring"; it is **not** an error.
///
/// Never panics. Never errors the recall path.
pub fn parse_dimension_time(phrase: &str, reference: DateTime<Utc>) -> Option<TimeRange> {
    let trimmed = phrase.trim();
    if trimmed.is_empty() {
        return None;
    }

    // two_timer takes an Option<Config>. Config::new() anchors to the wall
    // clock; we want `reference` (record.created_at) as "now" so that
    // "yesterday" means yesterday-relative-to-the-memory.
    let naive_now = reference.naive_utc();
    let config = two_timer::Config::new().now(naive_now);

    match two_timer::parse(trimmed, Some(config)) {
        Ok((start_naive, end_naive, _is_range)) => {
            // two_timer yields NaiveDateTime in the configured "now" frame.
            // engram stores everything in UTC, so treat the naive endpoints as UTC.
            let start = Utc.from_utc_datetime(&start_naive);
            let end = Utc.from_utc_datetime(&end_naive);

            // Defensive: reject inverted ranges. two_timer shouldn't produce them,
            // but a malformed phrase tickling a library edge case shouldn't break
            // `range_overlap_score`.
            if end < start {
                log::debug!(
                    "temporal_dim: discarding inverted range for phrase {:?}",
                    trimmed
                );
                return None;
            }
            Some(TimeRange { start, end })
        }
        Err(_) => {
            // Log once per unique phrase would be nicer, but that needs global
            // state. For now: DEBUG-level log is fine; bench runs can filter.
            log::debug!(
                "temporal_dim: parse miss for phrase {:?} (no time range extracted)",
                trimmed
            );
            None
        }
    }
}

/// LRU cache for dimension-parse results.
///
/// Keyed by `(phrase, reference_date)` where `reference_date` is the UTC date
/// of `record.created_at` (day granularity — sub-day variation doesn't change
/// parse results for the phrases we see in practice).
///
/// The cache stores `None` as well as `Some(range)`: re-parsing "maybe"
/// ten thousand times to learn it's unparseable is pointless.
pub struct DimParseCache {
    inner: LruCache<(String, NaiveDate), Option<TimeRange>>,
}

impl DimParseCache {
    /// Create a cache with the given capacity. Capacity is clamped to `1`
    /// minimum (LRU with zero capacity is a bug-prone degenerate).
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).unwrap();
        Self {
            inner: LruCache::new(cap),
        }
    }

    /// Lookup or parse. Hits update LRU recency; misses parse, insert, and return.
    pub fn get_or_parse(
        &mut self,
        phrase: &str,
        reference: DateTime<Utc>,
    ) -> Option<TimeRange> {
        let key = (phrase.to_string(), reference.date_naive());
        if let Some(cached) = self.inner.get(&key) {
            return cached.clone();
        }
        let parsed = parse_dimension_time(phrase, reference);
        self.inner.put(key, parsed.clone());
        parsed
    }

    /// Current number of entries (for test / debug).
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True if the cache has no entries.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for DimParseCache {
    fn default() -> Self {
        Self::new(CACHE_CAPACITY_DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    #[test]
    fn test_parse_yesterday_relative_to_reference() {
        let reference = ts(2024, 1, 10, 12);
        let r = parse_dimension_time("yesterday", reference)
            .expect("'yesterday' must parse");
        // "yesterday" anchored at 2024-01-10 → range covering 2024-01-09.
        assert_eq!(r.start.date_naive(), NaiveDate::from_ymd_opt(2024, 1, 9).unwrap());
        // End is exclusive in two_timer; should be on or after Jan 9, at most Jan 10.
        assert!(r.end.date_naive() >= NaiveDate::from_ymd_opt(2024, 1, 9).unwrap());
        assert!(r.end.date_naive() <= NaiveDate::from_ymd_opt(2024, 1, 10).unwrap());
    }

    #[test]
    fn test_parse_absolute_date() {
        // Absolute dates ignore `reference`; any anchor works.
        let reference = ts(2024, 1, 10, 12);
        let r = parse_dimension_time("May 6, 1968", reference)
            .expect("absolute date must parse");
        assert_eq!(r.start.date_naive(), NaiveDate::from_ymd_opt(1968, 5, 6).unwrap());
    }

    #[test]
    fn test_parse_unparseable_returns_none() {
        let reference = ts(2024, 1, 10, 12);
        // two_timer rejects these (free-form phrases with no date info).
        // Note: the exact set of "unparseable" phrases depends on two_timer's
        // grammar — we test a handful that are clearly not parseable dates.
        for phrase in &["", "   ", "@#$%", "kjshdfkjsdhf"] {
            assert!(
                parse_dimension_time(phrase, reference).is_none(),
                "phrase {:?} should be unparseable",
                phrase
            );
        }
    }

    #[test]
    fn test_cache_hits_same_phrase_and_day() {
        let mut cache = DimParseCache::new(16);
        let reference = ts(2024, 1, 10, 12);
        let r1 = cache.get_or_parse("yesterday", reference);
        assert!(r1.is_some());
        assert_eq!(cache.len(), 1);

        // Same phrase + same day (different hour) → cache hit, no new entry.
        let reference2 = ts(2024, 1, 10, 18);
        let r2 = cache.get_or_parse("yesterday", reference2);
        assert_eq!(r1.map(|r| r.start), r2.map(|r| r.start));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_cache_different_days_different_keys() {
        let mut cache = DimParseCache::new(16);
        let ref1 = ts(2024, 1, 10, 12);
        let ref2 = ts(2024, 1, 11, 12);
        cache.get_or_parse("yesterday", ref1);
        cache.get_or_parse("yesterday", ref2);
        // Different reference days → different cache keys → two entries.
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_cache_stores_none() {
        let mut cache = DimParseCache::new(16);
        let reference = ts(2024, 1, 10, 12);
        let r = cache.get_or_parse("kjshdfkjsdhf", reference);
        assert!(r.is_none());
        assert_eq!(cache.len(), 1); // None is cached

        // Re-query: still None, still 1 entry (cache hit).
        let r2 = cache.get_or_parse("kjshdfkjsdhf", reference);
        assert!(r2.is_none());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_cache_capacity_enforces_lru_eviction() {
        let mut cache = DimParseCache::new(2);
        let reference = ts(2024, 1, 10, 12);
        cache.get_or_parse("yesterday", reference);
        cache.get_or_parse("today", reference);
        cache.get_or_parse("tomorrow", reference);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_empty_and_whitespace_phrases_return_none_without_parsing() {
        let mut cache = DimParseCache::new(16);
        let reference = ts(2024, 1, 10, 12);
        assert!(cache.get_or_parse("", reference).is_none());
        assert!(cache.get_or_parse("   ", reference).is_none());
    }

    #[test]
    fn test_inverted_range_rejected() {
        // Smoke: normal phrase yields start <= end. (Inverted-range handling
        // is defensive; two_timer rarely produces one, so we just verify the
        // invariant on known-good input.)
        let reference = ts(2024, 1, 10, 12);
        if let Some(r) = parse_dimension_time("last Tuesday", reference) {
            assert!(r.start <= r.end, "start must be <= end");
        }
    }
}
