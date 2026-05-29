//! Temporal grounding for ExtractedFact text fields.
//!
//! Rewrites resolved relative time expressions (e.g. "yesterday") inline
//! with their absolute date ("yesterday (2024-03-14)") so retrieval content
//! carries temporal anchors. See ISS-088.
//!
//! ## Design notes
//!
//! - Only `core_fact` has its original text preserved (in
//!   `GroundingResult.original_core_fact`). The `temporal` and `context`
//!   fields are rewritten in place without preservation: they are
//!   metadata-derived and the absolute-date form is strictly more useful
//!   downstream. Keeping a separate "original" copy of every dimensional
//!   field would balloon `user_metadata` for marginal recall benefit.
//!
//! - Detection uses a curated list of relative-time phrase patterns. We
//!   do NOT try to detect arbitrary temporal expressions: any phrase that
//!   matches our regex is then re-validated by `parse_dimension_time`
//!   (powered by `two_timer`). If two_timer can't resolve it, we skip.
//!
//! - Rewrites are applied right-to-left so earlier byte offsets stay
//!   valid as later spans grow.

use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use regex::Regex;

use crate::extractor::ExtractedFact;
use crate::temporal_dim::parse_dimension_time;

/// Outcome of grounding one fact.
#[derive(Debug, Clone, Default)]
pub struct GroundingResult {
    /// True iff at least one phrase in any text field was rewritten.
    pub modified: bool,
    /// The original `core_fact` before any rewrite. Only set when
    /// `modified` and `core_fact` itself was rewritten.
    pub original_core_fact: Option<String>,
}

/// Curated relative-time phrase regex (case-insensitive, word-bounded).
///
/// Pattern groups:
/// - single-word: yesterday, today, tomorrow
/// - "<last|this|next> <week|month|year>"
/// - "N (days|weeks) ago"
///
/// ISS-190: the `months`/`years` arms were REMOVED. `two_timer` cannot
/// resolve "N months ago" / "N years ago" (it returns Err), so the regex
/// matched them only for `parse_dimension_time` to reject them — a silent
/// dead branch. Multi-month/year durations are now resolved at extraction
/// time by the reference-date-aware LLM (see `extractor::reference_preamble`),
/// not by this rule library. `two_timer` stays a fallback for the short-range
/// deixis it *can* handle.
fn phrase_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?ix)
            \b(
                yesterday
              | today
              | tomorrow
              | (?:last|this|next)\s+(?:week|month|year)
              | \d+\s+(?:days?|weeks?)\s+ago
            )\b
            ",
        )
        .expect("temporal_grounding: phrase regex must compile")
    })
}

/// True iff `tail` already begins with a date annotation of the form
/// `" (YYYY-MM-DD)"`. Used to skip already-grounded occurrences.
fn already_annotated(tail: &str) -> bool {
    let bytes = tail.as_bytes();
    if bytes.len() < 13 {
        return false;
    }
    // " (YYYY-MM-DD)"
    if bytes[0] != b' ' || bytes[1] != b'(' {
        return false;
    }
    let digits_dashes = &bytes[2..12];
    // YYYY-MM-DD
    if !digits_dashes[0].is_ascii_digit()
        || !digits_dashes[1].is_ascii_digit()
        || !digits_dashes[2].is_ascii_digit()
        || !digits_dashes[3].is_ascii_digit()
        || digits_dashes[4] != b'-'
        || !digits_dashes[5].is_ascii_digit()
        || !digits_dashes[6].is_ascii_digit()
        || digits_dashes[7] != b'-'
        || !digits_dashes[8].is_ascii_digit()
        || !digits_dashes[9].is_ascii_digit()
    {
        return false;
    }
    bytes[12] == b')'
}

/// Apply grounding to a single text field. Returns `true` iff the field
/// was modified.
fn ground_field(field: &mut String, reference: DateTime<Utc>) -> bool {
    let re = phrase_regex();

    // Collect non-overlapping leftmost-first candidate spans.
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut last_end = 0;
    for m in re.find_iter(field) {
        if m.start() < last_end {
            // overlapping with prior — skip (find_iter is already
            // non-overlapping, but be defensive).
            continue;
        }
        spans.push((m.start(), m.end()));
        last_end = m.end();
    }

    // Plan rewrites: (start, end, replacement). Skip if already annotated
    // or if two_timer can't resolve.
    let mut rewrites: Vec<(usize, usize, String)> = Vec::new();
    for (s, e) in spans.iter().copied() {
        // Idempotency: peek at chars after the match.
        let tail_end = (e + 13).min(field.len());
        // Ensure we don't slice mid-codepoint. Bytes at e..tail_end are
        // ASCII for our annotation pattern; if not, `already_annotated`
        // simply returns false because of the strict ASCII checks.
        let tail = match field.get(e..tail_end) {
            Some(t) => t,
            None => "",
        };
        if already_annotated(tail) {
            continue;
        }

        let phrase = &field[s..e];
        let range = match parse_dimension_time(phrase, reference) {
            Some(r) => r,
            None => continue,
        };
        let date = range.start.date_naive();
        let replacement = format!("{} ({})", phrase, date.format("%Y-%m-%d"));
        rewrites.push((s, e, replacement));
    }

    if rewrites.is_empty() {
        return false;
    }

    // Apply right-to-left.
    for (s, e, replacement) in rewrites.into_iter().rev() {
        field.replace_range(s..e, &replacement);
    }
    true
}

/// Rewrite resolved relative-time phrases in a fact's text fields,
/// anchored to `reference`.
///
/// Mutates these `ExtractedFact` fields in place: `core_fact`,
/// `temporal`, `context`. Each detected phrase like "yesterday" is
/// replaced with `"yesterday (2024-03-14)"` where the date is the
/// `start.date_naive()` of the resolved `TimeRange`.
///
/// Returns a `GroundingResult` so the caller can stash the original
/// `core_fact` in `StorageMeta.user_metadata` under `"original_content"`.
///
/// **Originals preservation**: only `core_fact`'s pre-rewrite value is
/// preserved (it becomes `MemoryRecord.content`, the field most likely
/// to be surfaced verbatim to the LLM). `temporal` / `context` are
/// rewritten without saving the original — by design (see module docs).
pub fn ground_fact(fact: &mut ExtractedFact, reference: DateTime<Utc>) -> GroundingResult {
    let original_core = fact.core_fact.clone();

    let core_changed = ground_field(&mut fact.core_fact, reference);

    let temporal_changed = if let Some(t) = fact.temporal.as_mut() {
        ground_field(t, reference)
    } else {
        false
    };

    let context_changed = if let Some(c) = fact.context.as_mut() {
        ground_field(c, reference)
    } else {
        false
    };

    let modified = core_changed || temporal_changed || context_changed;
    GroundingResult {
        modified,
        original_core_fact: if core_changed { Some(original_core) } else { None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 10, 0, 0).unwrap()
    }

    #[test]
    fn iss190_regex_no_longer_matches_unresolvable_month_year_durations() {
        // AC-4: the dead `months?|years?` regex arms are removed. two_timer
        // cannot resolve these, so matching them only produced silent skips.
        // Multi-month/year derivation now happens in the extractor LLM.
        let re = phrase_regex();
        assert!(!re.is_match("owned for 3 years ago"), "years-ago must not match");
        assert!(!re.is_match("2 months ago"), "months-ago must not match");
        // The short-range deixis two_timer CAN handle is still matched.
        assert!(re.is_match("3 days ago"), "days-ago must still match");
        assert!(re.is_match("2 weeks ago"), "weeks-ago must still match");
        assert!(re.is_match("yesterday"), "yesterday must still match");
    }

    #[test]
    fn iss190_year_duration_field_is_left_untouched() {
        // The exact q29 topology: a year-scale duration in the field must NOT
        // be ground here (the LLM owns it now) — and must NOT be silently
        // half-processed. ground_field returns false (no rewrite).
        let mut field = String::from("owned for 3 years");
        let changed = ground_field(&mut field, ts(2023, 3, 27));
        assert!(!changed, "year-scale duration must not be touched by grounding");
        assert_eq!(field, "owned for 3 years", "field must be unchanged");
    }

    #[test]
    fn grounds_yesterday_in_core_fact() {
        let mut fact = ExtractedFact {
            core_fact: "user attended support group yesterday".into(),
            ..Default::default()
        };
        let reference = ts(2024, 3, 15);
        let r = ground_fact(&mut fact, reference);
        assert_eq!(
            fact.core_fact,
            "user attended support group yesterday (2024-03-14)"
        );
        assert!(r.modified);
        assert_eq!(
            r.original_core_fact.as_deref(),
            Some("user attended support group yesterday")
        );
    }

    #[test]
    fn grounds_multiple_phrases_leftmost_first_no_offset_drift() {
        let mut fact = ExtractedFact {
            core_fact: "saw doctor yesterday and gym last week".into(),
            ..Default::default()
        };
        let reference = ts(2024, 3, 15);
        let r = ground_fact(&mut fact, reference);
        assert!(r.modified);
        // yesterday → 2024-03-14. "last week" anchored at 2024-03-15
        // (a Friday) → previous calendar week per two_timer, which
        // typically resolves to the Mon..Sun week before. The exact
        // start date varies with locale, so we assert structure rather
        // than exact date for the second annotation.
        assert!(
            fact.core_fact.starts_with("saw doctor yesterday (2024-03-14) and gym last week ("),
            "got: {}",
            fact.core_fact
        );
        assert!(fact.core_fact.ends_with(")"));
        // No garbled offsets — both phrases retained verbatim before the paren.
        assert!(fact.core_fact.contains("yesterday (2024-03-14)"));
        assert!(fact.core_fact.contains("last week ("));
    }

    #[test]
    fn idempotent_does_not_double_annotate() {
        let mut fact = ExtractedFact {
            core_fact: "yesterday (2024-03-14) ate sushi".into(),
            ..Default::default()
        };
        let reference = ts(2024, 3, 15);
        let r = ground_fact(&mut fact, reference);
        assert_eq!(fact.core_fact, "yesterday (2024-03-14) ate sushi");
        assert!(!r.modified);
        assert!(r.original_core_fact.is_none());
    }

    #[test]
    fn unparseable_phrase_skipped() {
        let mut fact = ExtractedFact {
            core_fact: "the day after the big event".into(),
            ..Default::default()
        };
        let reference = ts(2024, 3, 15);
        let r = ground_fact(&mut fact, reference);
        assert_eq!(fact.core_fact, "the day after the big event");
        assert!(!r.modified);
    }

    #[test]
    fn grounds_temporal_and_context_fields_too() {
        let mut fact = ExtractedFact {
            core_fact: "ate pizza".into(),
            temporal: Some("yesterday".into()),
            context: Some("last week we discussed it".into()),
            ..Default::default()
        };
        let reference = ts(2024, 3, 15);
        let r = ground_fact(&mut fact, reference);
        assert!(r.modified);
        assert_eq!(fact.temporal.as_deref(), Some("yesterday (2024-03-14)"));
        assert!(
            fact.context
                .as_deref()
                .unwrap()
                .starts_with("last week ("),
            "got: {:?}",
            fact.context
        );
        // core_fact wasn't rewritten → original_core_fact is None.
        assert!(r.original_core_fact.is_none());
        assert_eq!(fact.core_fact, "ate pizza");
    }

    #[test]
    fn handles_empty_optional_fields() {
        let mut fact = ExtractedFact {
            core_fact: "no time refs here".into(),
            temporal: None,
            context: None,
            ..Default::default()
        };
        let reference = ts(2024, 3, 15);
        let r = ground_fact(&mut fact, reference);
        assert!(!r.modified);
    }
}
