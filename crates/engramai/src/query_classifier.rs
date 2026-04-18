//! Query type classification for adaptive weight adjustment.
//!
//! Classifies queries into Temporal, Keyword, Semantic, or General types
//! using heuristics (no LLM). Each type produces weight modifiers that
//! boost or reduce the contribution of each retrieval channel.

use chrono::{DateTime, Datelike, Duration, NaiveTime, Utc, TimeZone};

/// Detected query type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryType {
    /// Time-related: "what happened yesterday?", "昨天的会议"
    Temporal,
    /// Short exact terms: "RustClaw v0.1.0", "error code E0308"
    Keyword,
    /// Natural language: "how does the memory system work?"
    Semantic,
    /// Mixed or unclear
    General,
}

/// Result of query analysis.
#[derive(Debug, Clone)]
pub struct QueryAnalysis {
    /// Detected query type
    pub query_type: QueryType,
    /// Suggested weight multipliers per channel (1.0 = no change)
    pub weight_modifiers: ChannelWeightModifiers,
    /// Extracted time range if temporal query detected
    pub time_range: Option<TimeRange>,
}

/// Per-channel weight multipliers applied to base config weights.
#[derive(Debug, Clone)]
pub struct ChannelWeightModifiers {
    pub fts: f64,
    pub embedding: f64,
    pub actr: f64,
    pub entity: f64,
    pub temporal: f64,
    pub hebbian: f64,
}

/// A time range for temporal filtering.
#[derive(Debug, Clone)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl QueryAnalysis {
    /// Neutral analysis — all modifiers at 1.0, no time range.
    pub fn neutral() -> Self {
        Self {
            query_type: QueryType::General,
            weight_modifiers: ChannelWeightModifiers::neutral(),
            time_range: None,
        }
    }
}

impl ChannelWeightModifiers {
    /// All modifiers at 1.0 (no adjustment).
    pub fn neutral() -> Self {
        Self {
            fts: 1.0,
            embedding: 1.0,
            actr: 1.0,
            entity: 1.0,
            temporal: 1.0,
            hebbian: 1.0,
        }
    }

    fn for_temporal() -> Self {
        Self {
            fts: 1.0,
            embedding: 0.5,
            actr: 2.0,
            entity: 1.0,
            temporal: 3.0,
            hebbian: 1.0,
        }
    }

    fn for_keyword() -> Self {
        Self {
            fts: 3.0,
            embedding: 0.5,
            actr: 1.0,
            entity: 2.0,
            temporal: 1.0,
            hebbian: 1.0,
        }
    }

    fn for_semantic() -> Self {
        Self {
            fts: 1.0,
            embedding: 1.5,
            actr: 1.0,
            entity: 1.0,
            temporal: 1.0,
            hebbian: 1.0,
        }
    }
}

// ── Temporal patterns ──────────────────────────────────────────────

/// English temporal indicators
const TEMPORAL_EN: &[&str] = &[
    "yesterday", "today", "last week", "last month", "last year",
    "this week", "this month", "this year",
    "ago", "recently", "earlier", "before", "previous",
    "last night", "this morning", "tonight",
    "hours ago", "days ago", "weeks ago", "months ago",
];

/// Chinese temporal indicators
const TEMPORAL_ZH: &[&str] = &[
    "昨天", "今天", "上周", "上个月", "去年",
    "这周", "这个月", "今年",
    "前天", "大前天", "刚才", "之前",
    "几天前", "几周前", "几个月前",
    "上午", "下午", "晚上", "早上",
];

/// Regex-like patterns for "N days/hours/weeks ago"
/// Result of "N units ago" detection. Unit is owned to avoid lifetime issues.
#[derive(Debug, Clone)]
struct NUnitAgo {
    n: i64,
    unit: String,
}

fn has_n_unit_ago(query: &str) -> Option<NUnitAgo> {
    let lower = query.to_lowercase();
    // Match patterns like "3 days ago", "2 weeks ago", "1 hour ago"
    let words: Vec<&str> = lower.split_whitespace().collect();
    for window in words.windows(3) {
        if window[2] == "ago" {
            if let Ok(n) = window[0].parse::<i64>() {
                let unit = window[1].trim_end_matches('s');
                match unit {
                    "day" | "week" | "month" | "hour" | "minute" => {
                        return Some(NUnitAgo { n, unit: unit.to_string() });
                    }
                    _ => {}
                }
            }
        }
    }
    // Chinese: "N天前", "N周前", "N个月前"
    for ch_pattern in &["天前", "周前", "个月前", "小时前"] {
        if let Some(pos) = query.find(ch_pattern) {
            // Try to parse the number before the pattern
            let prefix = &query[..pos];
            // Get the last few characters that could be a number
            let num_str: String = prefix.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<String>().chars().rev().collect();
            if let Ok(n) = num_str.parse::<i64>() {
                let unit = match *ch_pattern {
                    "天前" => "day",
                    "周前" => "week",
                    "个月前" => "month",
                    "小时前" => "hour",
                    _ => "day",
                };
                return Some(NUnitAgo { n, unit: unit.to_string() });
            }
        }
    }
    None
}

fn is_temporal_query(query: &str) -> bool {
    let lower = query.to_lowercase();
    for pattern in TEMPORAL_EN {
        if lower.contains(pattern) {
            return true;
        }
    }
    for pattern in TEMPORAL_ZH {
        if query.contains(pattern) {
            return true;
        }
    }
    if has_n_unit_ago(query).is_some() {
        return true;
    }
    // ISO date patterns: 2024-01-15, 2024/01/15
    let has_iso = query.chars().any(|c| c == '-' || c == '/')
        && query.chars().filter(|c| c.is_ascii_digit()).count() >= 8;
    if has_iso {
        // More specific check: YYYY-MM-DD or YYYY/MM/DD
        for word in query.split_whitespace() {
            if word.len() >= 8 && word.len() <= 10 {
                let digits: String = word.chars().filter(|c| c.is_ascii_digit()).collect();
                if digits.len() == 8 {
                    return true;
                }
            }
        }
    }
    false
}

// ── Keyword patterns ───────────────────────────────────────────────

fn is_keyword_query(query: &str) -> bool {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.len() > 4 {
        return false;
    }
    // Check for identifiers: CamelCase, snake_case, version numbers, error codes
    for word in &words {
        // CamelCase: has uppercase letters after lowercase
        if word.chars().any(|c| c.is_uppercase())
            && word.chars().any(|c| c.is_lowercase())
            && !word.starts_with(|c: char| c.is_uppercase())
                || has_camel_case(word)
        {
            return true;
        }
        // snake_case
        if word.contains('_') && word.chars().any(|c| c.is_alphanumeric()) {
            return true;
        }
        // Version numbers: v1.0.0, 0.1.0
        if word.starts_with('v') && word[1..].contains('.') && word[1..].chars().any(|c| c.is_ascii_digit()) {
            return true;
        }
        if word.chars().filter(|c| *c == '.').count() >= 1
            && word.chars().filter(|c| c.is_ascii_digit()).count() >= 2
            && word.len() <= 12
        {
            return true;
        }
        // Error codes: E0308, ERR_123
        if word.len() >= 3
            && word.starts_with(|c: char| c.is_uppercase())
            && word[1..].chars().any(|c| c.is_ascii_digit())
            && !word.contains(' ')
        {
            return true;
        }
    }
    false
}

fn has_camel_case(word: &str) -> bool {
    let mut saw_lower = false;
    for c in word.chars() {
        if c.is_lowercase() {
            saw_lower = true;
        } else if c.is_uppercase() && saw_lower {
            return true;
        }
    }
    false
}

// ── Semantic detection ─────────────────────────────────────────────

fn is_semantic_query(query: &str) -> bool {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.len() < 5 {
        return false;
    }
    // Contains question words or natural language structure
    let lower = query.to_lowercase();
    let question_words = ["how", "what", "why", "when", "where", "which", "who",
                          "explain", "describe", "tell me", "can you",
                          "怎么", "什么", "为什么", "如何", "哪个", "谁"];
    for qw in &question_words {
        if lower.contains(qw) {
            return true;
        }
    }
    // Long queries are likely semantic even without question words
    words.len() >= 7
}

// ── Time range extraction ──────────────────────────────────────────

fn extract_time_range(query: &str) -> Option<TimeRange> {
    let now = Utc::now();
    let lower = query.to_lowercase();

    // "yesterday" / "昨天"
    if lower.contains("yesterday") || query.contains("昨天") {
        let yesterday = now - Duration::days(1);
        let start = yesterday.date_naive().and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        let end = yesterday.date_naive().and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap());
        return Some(TimeRange {
            start: Utc.from_utc_datetime(&start),
            end: Utc.from_utc_datetime(&end),
        });
    }

    // "today" / "今天"
    if lower.contains("today") || query.contains("今天") {
        let start = now.date_naive().and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        return Some(TimeRange {
            start: Utc.from_utc_datetime(&start),
            end: now,
        });
    }

    // "前天" (day before yesterday)
    if query.contains("前天") && !query.contains("大前天") {
        let day = now - Duration::days(2);
        let start = day.date_naive().and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        let end = day.date_naive().and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap());
        return Some(TimeRange {
            start: Utc.from_utc_datetime(&start),
            end: Utc.from_utc_datetime(&end),
        });
    }

    // "last week" / "上周"
    if lower.contains("last week") || query.contains("上周") {
        return Some(TimeRange {
            start: now - Duration::weeks(1),
            end: now,
        });
    }

    // "last month" / "上个月"
    if lower.contains("last month") || query.contains("上个月") {
        return Some(TimeRange {
            start: now - Duration::days(30),
            end: now,
        });
    }

    // "this week" / "这周"
    if lower.contains("this week") || query.contains("这周") {
        // From start of current week (Monday) to now
        let weekday = now.date_naive().weekday().num_days_from_monday();
        let start = (now - Duration::days(weekday as i64)).date_naive()
            .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        return Some(TimeRange {
            start: Utc.from_utc_datetime(&start),
            end: now,
        });
    }

    // N units ago
    if let Some(nua) = has_n_unit_ago(query) {
        let duration = match nua.unit.as_str() {
            "day" => Duration::days(nua.n),
            "week" => Duration::weeks(nua.n),
            "month" => Duration::days(nua.n * 30),
            "hour" => Duration::hours(nua.n),
            "minute" => Duration::minutes(nua.n),
            _ => Duration::days(nua.n),
        };
        return Some(TimeRange {
            start: now - duration,
            end: now,
        });
    }

    // "recently" / "刚才"
    if lower.contains("recently") || query.contains("刚才") {
        return Some(TimeRange {
            start: now - Duration::hours(24),
            end: now,
        });
    }

    None
}

// ── Main classifier ────────────────────────────────────────────────

/// Classify a query and produce weight modifiers + optional time range.
pub fn classify_query(query: &str) -> QueryAnalysis {
    // Check temporal first (highest priority — temporal queries often contain other signals)
    if is_temporal_query(query) {
        return QueryAnalysis {
            query_type: QueryType::Temporal,
            weight_modifiers: ChannelWeightModifiers::for_temporal(),
            time_range: extract_time_range(query),
        };
    }

    // Then keyword
    if is_keyword_query(query) {
        return QueryAnalysis {
            query_type: QueryType::Keyword,
            weight_modifiers: ChannelWeightModifiers::for_keyword(),
            time_range: None,
        };
    }

    // Then semantic
    if is_semantic_query(query) {
        return QueryAnalysis {
            query_type: QueryType::Semantic,
            weight_modifiers: ChannelWeightModifiers::for_semantic(),
            time_range: None,
        };
    }

    // Default: general
    QueryAnalysis::neutral()
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_temporal_english() {
        let analysis = classify_query("what happened yesterday");
        assert_eq!(analysis.query_type, QueryType::Temporal);
        assert!(analysis.time_range.is_some());
    }

    #[test]
    fn test_classify_temporal_chinese() {
        let analysis = classify_query("昨天potato说了什么");
        assert_eq!(analysis.query_type, QueryType::Temporal);
        assert!(analysis.time_range.is_some());
    }

    #[test]
    fn test_classify_keyword() {
        let analysis = classify_query("RustClaw v0.1.0");
        assert_eq!(analysis.query_type, QueryType::Keyword);
        assert!(analysis.time_range.is_none());
    }

    #[test]
    fn test_classify_semantic() {
        let analysis = classify_query("how does the memory consolidation system work");
        assert_eq!(analysis.query_type, QueryType::Semantic);
        assert!(analysis.time_range.is_none());
    }

    #[test]
    fn test_classify_general() {
        let analysis = classify_query("hello");
        assert_eq!(analysis.query_type, QueryType::General);
        assert!(analysis.time_range.is_none());
    }

    #[test]
    fn test_time_range_yesterday() {
        let analysis = classify_query("what happened yesterday");
        let range = analysis.time_range.unwrap();
        let now = Utc::now();
        let yesterday = now - Duration::days(1);
        // Start should be yesterday 00:00
        assert_eq!(range.start.date_naive(), yesterday.date_naive());
        assert_eq!(range.start.time(), NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        // End should be yesterday 23:59:59
        assert_eq!(range.end.date_naive(), yesterday.date_naive());
        assert_eq!(range.end.time(), NaiveTime::from_hms_opt(23, 59, 59).unwrap());
    }

    #[test]
    fn test_time_range_last_week() {
        let analysis = classify_query("what did we discuss last week");
        let range = analysis.time_range.unwrap();
        let now = Utc::now();
        // Start should be ~7 days ago
        let diff = now - range.start;
        assert!(diff.num_days() >= 6 && diff.num_days() <= 8,
            "last week start should be ~7 days ago, got {} days", diff.num_days());
        // End should be close to now
        let end_diff = now - range.end;
        assert!(end_diff.num_seconds() < 5, "end should be close to now");
    }

    #[test]
    fn test_time_range_n_days_ago() {
        let analysis = classify_query("what was discussed 3 days ago");
        let range = analysis.time_range.unwrap();
        let now = Utc::now();
        // Start should be ~3 days ago
        let diff = now - range.start;
        assert!(diff.num_days() >= 2 && diff.num_days() <= 4,
            "3 days ago start should be ~3 days, got {} days", diff.num_days());
    }

    #[test]
    fn test_weight_modifiers_temporal() {
        let analysis = classify_query("what happened yesterday");
        let m = &analysis.weight_modifiers;
        assert!(m.temporal > 2.0, "temporal should be boosted: {}", m.temporal);
        assert!(m.actr > 1.0, "actr should be boosted: {}", m.actr);
        assert!(m.embedding < 1.0, "embedding should be reduced: {}", m.embedding);
    }

    #[test]
    fn test_neutral_analysis() {
        let analysis = QueryAnalysis::neutral();
        let m = &analysis.weight_modifiers;
        assert_eq!(m.fts, 1.0);
        assert_eq!(m.embedding, 1.0);
        assert_eq!(m.actr, 1.0);
        assert_eq!(m.entity, 1.0);
        assert_eq!(m.temporal, 1.0);
        assert_eq!(m.hebbian, 1.0);
        assert!(analysis.time_range.is_none());
    }

    #[test]
    fn test_classify_temporal_n_days_ago_chinese() {
        let analysis = classify_query("3天前发生了什么");
        assert_eq!(analysis.query_type, QueryType::Temporal);
        assert!(analysis.time_range.is_some());
    }

    #[test]
    fn test_classify_keyword_error_code() {
        let analysis = classify_query("E0308");
        assert_eq!(analysis.query_type, QueryType::Keyword);
    }

    #[test]
    fn test_classify_keyword_snake_case() {
        let analysis = classify_query("memory_config");
        assert_eq!(analysis.query_type, QueryType::Keyword);
    }
}
