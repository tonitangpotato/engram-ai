//! Query type classification for adaptive weight adjustment.
//!
//! Classifies queries into Temporal, Keyword, Semantic, or General types
//! using heuristics (no LLM). Each type produces weight modifiers that
//! boost or reduce the contribution of each retrieval channel.
//!
//! Also classifies **query intent** (Definition, HowTo, Event, Relational,
//! Context, General) for type-affinity modulation — boosting memories
//! whose `MemoryType` matches the intent behind the query.

use chrono::{DateTime, Datelike, Duration, NaiveTime, Utc, TimeZone};
use regex::RegexSet;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::anthropic_client::{build_anthropic_headers, DEFAULT_ANTHROPIC_API_URL};

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

/// Query intent — what kind of information the user is looking for.
///
/// Used for type-affinity modulation: each intent maps to a set of
/// multipliers that boost or suppress different `MemoryType`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryIntent {
    /// "what is X", "X 是什么" → wants facts/definitions
    Definition,
    /// "how to X", "怎么做" → wants procedures/instructions
    HowTo,
    /// "what happened", "发生了什么" → wants episodic/event memories
    Event,
    /// "relationship between", "关系", "who knows" → wants relational knowledge
    Relational,
    /// "I'm working on X", "我在搞" → working context, non-question
    Context,
    /// Fallback — no strong intent signal
    General,
}

/// Per-MemoryType affinity multipliers for a given query intent.
///
/// These are applied as multiplicative modulation on the combined 7-channel
/// score. A multiplier of 1.0 means no change; >1 boosts, <1 suppresses.
#[derive(Debug, Clone)]
pub struct TypeAffinity {
    pub factual: f64,
    pub episodic: f64,
    pub relational: f64,
    pub emotional: f64,
    pub procedural: f64,
    pub opinion: f64,
    pub causal: f64,
}

impl TypeAffinity {
    /// Neutral affinity — all types at 1.0 (no modulation).
    pub fn neutral() -> Self {
        Self {
            factual: 1.0,
            episodic: 1.0,
            relational: 1.0,
            emotional: 1.0,
            procedural: 1.0,
            opinion: 1.0,
            causal: 1.0,
        }
    }
}

impl QueryIntent {
    /// Get the type-affinity multipliers for this intent.
    pub fn type_affinity(&self) -> TypeAffinity {
        match self {
            QueryIntent::Definition => TypeAffinity {
                factual: 2.0, episodic: 0.5, relational: 1.5,
                emotional: 0.3, procedural: 0.8, opinion: 0.5, causal: 0.8,
            },
            QueryIntent::HowTo => TypeAffinity {
                factual: 0.8, episodic: 0.5, relational: 0.5,
                emotional: 0.3, procedural: 2.5, opinion: 0.5, causal: 1.0,
            },
            QueryIntent::Event => TypeAffinity {
                factual: 0.5, episodic: 2.5, relational: 0.8,
                emotional: 1.5, procedural: 0.3, opinion: 0.5, causal: 0.8,
            },
            QueryIntent::Relational => TypeAffinity {
                factual: 0.8, episodic: 0.5, relational: 2.5,
                emotional: 1.5, procedural: 0.3, opinion: 1.5, causal: 0.8,
            },
            QueryIntent::Context => TypeAffinity {
                factual: 1.2, episodic: 1.8, relational: 1.0,
                emotional: 0.8, procedural: 1.0, opinion: 0.8, causal: 1.0,
            },
            QueryIntent::General => TypeAffinity::neutral(),
        }
    }
}

/// Result of query analysis.
#[derive(Debug, Clone)]
pub struct QueryAnalysis {
    /// Detected query type
    pub query_type: QueryType,
    /// Detected query intent (for type-affinity modulation)
    pub query_intent: QueryIntent,
    /// Suggested weight multipliers per channel (1.0 = no change)
    pub weight_modifiers: ChannelWeightModifiers,
    /// Per-MemoryType affinity multipliers derived from query intent
    pub type_affinity: TypeAffinity,
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
    pub somatic: f64,
}

/// A time range for temporal filtering.
#[derive(Debug, Clone)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl QueryAnalysis {
    /// Neutral analysis — all modifiers at 1.0, no time range, General intent.
    pub fn neutral() -> Self {
        Self {
            query_type: QueryType::General,
            query_intent: QueryIntent::General,
            weight_modifiers: ChannelWeightModifiers::neutral(),
            type_affinity: TypeAffinity::neutral(),
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
            somatic: 1.0,
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
            somatic: 1.0,
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
            somatic: 1.0,
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
            somatic: 1.0,
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

// ── Intent classification (Level 1: regex) ────────────────────────

/// Intent patterns for English queries.
const INTENT_DEFINITION_EN: &[&str] = &[
    "what is", "what are", "what does", "what's",
    "define", "definition of", "meaning of",
    "tell me about", "explain what",
    "who is", "who are",
];

const INTENT_HOWTO_EN: &[&str] = &[
    "how to", "how do i", "how do you", "how can i", "how should",
    "steps to", "tutorial", "instructions for",
    "guide to", "way to", "best way to",
    "how does", "how is",
];

const INTENT_EVENT_EN: &[&str] = &[
    "what happened", "what did", "what was discussed",
    "when did", "did we", "did i", "did you",
    "last time", "that time when",
    "history of", "timeline",
];

const INTENT_RELATIONAL_EN: &[&str] = &[
    "relationship between", "connection between",
    "who knows", "related to",
    "what do you think about", "opinion on", "opinion about",
    "how do you feel about", "what about",
    "between", // weaker signal, combined with other heuristics
];

const INTENT_CONTEXT_EN: &[&str] = &[
    "i'm working on", "i am working on", "working on",
    "currently building", "i'm building",
    "dealing with", "i'm dealing",
    "my project", "my task",
    "right now i", "at the moment",
    "(?i)i am (?:building|working)",
];

/// Intent patterns for Chinese queries.
const INTENT_DEFINITION_ZH: &[&str] = &[
    "是什么", "什么是", "定义", "意思是",
    "介绍一下", "解释一下", "说说",
    "什么书", "什么东西", "什么人",
    "用了什么", "用的什么", "用什么",
    "谁", "在哪", "哪里",
];

const INTENT_HOWTO_ZH: &[&str] = &[
    "怎么做", "怎么搞", "怎么弄", "如何",
    "步骤", "方法", "教程", "怎样",
    "操作", "指南",
    "怎么\\w+",
];

const INTENT_EVENT_ZH: &[&str] = &[
    "发生了什么", "发生了啥", "做了什么", "做了啥",
    "什么时候", "上次", "那次",
    "经历", "过去",
];

const INTENT_RELATIONAL_ZH: &[&str] = &[
    "关系", "认识", "之间",
    "觉得怎么样", "看法", "对.*的看法",
    "怎么看",
];

const INTENT_CONTEXT_ZH: &[&str] = &[
    "我在做", "我在搞", "正在做", "正在搞",
    "手上有", "目前在", "现在在",
];

// ── RegexSet-based intent matching ─────────────────────────────────

struct IntentRegex {
    howto_en: RegexSet,
    howto_zh: RegexSet,
    event_en: RegexSet,
    event_zh: RegexSet,
    definition_en: RegexSet,
    definition_zh: RegexSet,
    relational_en: RegexSet,
    relational_zh: RegexSet,
    context_en: RegexSet,
    context_zh: RegexSet,
}

static INTENT_REGEX: OnceLock<IntentRegex> = OnceLock::new();

fn get_intent_regex() -> &'static IntentRegex {
    INTENT_REGEX.get_or_init(|| {
        IntentRegex {
            howto_en: RegexSet::new(INTENT_HOWTO_EN).unwrap(),
            howto_zh: RegexSet::new(INTENT_HOWTO_ZH).unwrap(),
            event_en: RegexSet::new(INTENT_EVENT_EN).unwrap(),
            event_zh: RegexSet::new(INTENT_EVENT_ZH).unwrap(),
            definition_en: RegexSet::new(INTENT_DEFINITION_EN).unwrap(),
            definition_zh: RegexSet::new(INTENT_DEFINITION_ZH).unwrap(),
            relational_en: RegexSet::new(INTENT_RELATIONAL_EN).unwrap(),
            relational_zh: RegexSet::new(INTENT_RELATIONAL_ZH).unwrap(),
            context_en: RegexSet::new(INTENT_CONTEXT_EN).unwrap(),
            context_zh: RegexSet::new(INTENT_CONTEXT_ZH).unwrap(),
        }
    })
}

/// Classify query intent using RegexSet pattern matching (Level 1).
pub fn classify_intent_regex(query: &str) -> QueryIntent {
    let r = get_intent_regex();
    let lower = query.to_lowercase();

    if r.howto_en.is_match(&lower) || r.howto_zh.is_match(query) { return QueryIntent::HowTo; }
    if r.event_en.is_match(&lower) || r.event_zh.is_match(query) { return QueryIntent::Event; }
    if r.relational_en.is_match(&lower) || r.relational_zh.is_match(query) { return QueryIntent::Relational; }
    if r.definition_en.is_match(&lower) || r.definition_zh.is_match(query) { return QueryIntent::Definition; }
    if r.context_en.is_match(&lower) || r.context_zh.is_match(query) { return QueryIntent::Context; }
    QueryIntent::General
}

// ── Intent classification (Level 2: Haiku LLM) ────────────────────

/// Haiku-based intent classifier for Level 2 fallback.
///
/// When regex (Level 1) returns General, this classifier calls the
/// Anthropic API with a lightweight model to classify the query intent.
pub struct HaikuIntentClassifier {
    client: reqwest::blocking::Client,
    token_provider: Box<dyn crate::extractor::TokenProvider>,
    is_oauth: bool,
    model: String,
    api_url: String,
    #[allow(dead_code)]
    timeout_secs: u64,
    disabled: AtomicBool,
}

impl HaikuIntentClassifier {
    pub fn new(
        token_provider: Box<dyn crate::extractor::TokenProvider>,
        is_oauth: bool,
        model: String,
        api_url: Option<String>,
        timeout_secs: u64,
    ) -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .build()
                .unwrap_or_default(),
            token_provider,
            is_oauth,
            model,
            api_url: api_url.unwrap_or_else(|| DEFAULT_ANTHROPIC_API_URL.to_string()),
            timeout_secs,
            disabled: AtomicBool::new(false),
        }
    }

    pub fn classify(&self, query: &str) -> QueryIntent {
        if self.disabled.load(Ordering::Relaxed) {
            return QueryIntent::General;
        }

        let token = match self.token_provider.get_token() {
            Ok(t) => t,
            Err(_) => return QueryIntent::General,
        };

        let headers = build_anthropic_headers(&token, self.is_oauth);

        let prompt = format!(
            "Classify the intent of this query into EXACTLY ONE category.\n\
            Categories: definition, howto, event, relational, context, general\n\n\
            Rules:\n\
            - definition: asking what something IS, facts, descriptions\n\
            - howto: asking HOW to do something, steps, instructions\n\
            - event: asking what HAPPENED, when, history, timeline\n\
            - relational: asking about relationships, connections, opinions about\n\
            - context: stating what the speaker is working on (not a question)\n\
            - general: greetings, unclear, or doesn't fit above\n\n\
            The query may be in English or Chinese — classify based on meaning, not language.\n\n\
            Respond with ONLY the category name, nothing else.\n\n\
            Query: \"{}\"", query
        );

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 20,
            "messages": [{"role": "user", "content": prompt}]
        });

        let url = format!("{}/v1/messages", self.api_url);
        let resp = self.client.post(&url)
            .headers(headers)
            .json(&body)
            .send();

        match resp {
            Ok(r) => {
                if r.status() == reqwest::StatusCode::UNAUTHORIZED || r.status() == reqwest::StatusCode::FORBIDDEN {
                    self.disabled.store(true, Ordering::Relaxed);
                    log::warn!("HaikuIntentClassifier disabled: auth failure ({})", r.status());
                    return QueryIntent::General;
                }
                if !r.status().is_success() {
                    return QueryIntent::General;
                }
                // Parse Anthropic response
                if let Ok(json) = r.json::<serde_json::Value>() {
                    if let Some(text) = json["content"][0]["text"].as_str() {
                        return Self::parse_intent(text);
                    }
                }
                QueryIntent::General
            }
            Err(e) => {
                log::debug!("HaikuIntentClassifier L2 call failed: {}", e);
                QueryIntent::General
            }
        }
    }

    fn parse_intent(text: &str) -> QueryIntent {
        match text.trim().to_lowercase().as_str() {
            "definition" => QueryIntent::Definition,
            "howto" => QueryIntent::HowTo,
            "event" => QueryIntent::Event,
            "relational" => QueryIntent::Relational,
            "context" => QueryIntent::Context,
            "general" => QueryIntent::General,
            _ => QueryIntent::General,
        }
    }
}

// ── Main classifier ────────────────────────────────────────────────

/// Classify a query and produce weight modifiers + optional time range.
///
/// Uses regex-only intent classification (Level 1).
/// For Level 2 Haiku-based intent classification, use
/// [`classify_query_with_l2`].
pub fn classify_query(query: &str) -> QueryAnalysis {
    let intent = classify_intent_regex(query);
    let type_affinity = intent.type_affinity();
    classify_query_inner(query, intent, type_affinity)
}

/// Classify a query with optional Haiku L2 intent fallback.
///
/// Two-level intent classification:
/// - Level 1: RegexSet pattern matching (always runs)
/// - Level 2: if regex returns General and `haiku_classifier` is provided,
///   call the Anthropic API with a lightweight model
///
/// # Arguments
///
/// * `query` - The natural language query
/// * `haiku_classifier` - Optional HaikuIntentClassifier for Level 2 fallback
pub fn classify_query_with_l2(
    query: &str,
    haiku_classifier: Option<&HaikuIntentClassifier>,
) -> QueryAnalysis {
    let mut intent = classify_intent_regex(query);

    if intent == QueryIntent::General {
        if let Some(classifier) = haiku_classifier {
            intent = classifier.classify(query);
        }
    }

    let type_affinity = intent.type_affinity();
    classify_query_inner(query, intent, type_affinity)
}

/// Inner classifier: determines QueryType and builds the full QueryAnalysis.
fn classify_query_inner(query: &str, intent: QueryIntent, type_affinity: TypeAffinity) -> QueryAnalysis {
    // Check temporal first (highest priority — temporal queries often contain other signals)
    if is_temporal_query(query) {
        return QueryAnalysis {
            query_type: QueryType::Temporal,
            query_intent: intent,
            weight_modifiers: ChannelWeightModifiers::for_temporal(),
            type_affinity,
            time_range: extract_time_range(query),
        };
    }

    // Then keyword
    if is_keyword_query(query) {
        return QueryAnalysis {
            query_type: QueryType::Keyword,
            query_intent: intent,
            weight_modifiers: ChannelWeightModifiers::for_keyword(),
            type_affinity,
            time_range: None,
        };
    }

    // Then semantic
    if is_semantic_query(query) {
        return QueryAnalysis {
            query_type: QueryType::Semantic,
            query_intent: intent,
            weight_modifiers: ChannelWeightModifiers::for_semantic(),
            type_affinity,
            time_range: None,
        };
    }

    // Default: general
    QueryAnalysis {
        query_type: QueryType::General,
        query_intent: intent,
        weight_modifiers: ChannelWeightModifiers::neutral(),
        type_affinity,
        time_range: None,
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::needless_borrows_for_generic_args)]
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

    // ── Intent classification tests ────────────────────────────────

    #[test]
    fn test_intent_definition_english() {
        let analysis = classify_query("what is ACT-R activation");
        assert_eq!(analysis.query_intent, QueryIntent::Definition);
    }

    #[test]
    fn test_intent_definition_chinese() {
        let analysis = classify_query("embedding是什么");
        assert_eq!(analysis.query_intent, QueryIntent::Definition);
    }

    #[test]
    fn test_intent_howto_english() {
        let analysis = classify_query("how to configure the memory system for best results");
        assert_eq!(analysis.query_intent, QueryIntent::HowTo);
    }

    #[test]
    fn test_intent_howto_chinese() {
        let analysis = classify_query("怎么做数据库迁移");
        assert_eq!(analysis.query_intent, QueryIntent::HowTo);
    }

    #[test]
    fn test_intent_event_english() {
        let analysis = classify_query("what happened in the last meeting");
        assert_eq!(analysis.query_intent, QueryIntent::Event);
    }

    #[test]
    fn test_intent_event_chinese() {
        let analysis = classify_query("上次发生了什么");
        assert_eq!(analysis.query_intent, QueryIntent::Event);
    }

    #[test]
    fn test_intent_relational_english() {
        let analysis = classify_query("what is the relationship between potato and engram");
        assert_eq!(analysis.query_intent, QueryIntent::Relational);
    }

    #[test]
    fn test_intent_relational_chinese() {
        let analysis = classify_query("potato和小明的关系");
        assert_eq!(analysis.query_intent, QueryIntent::Relational);
    }

    #[test]
    fn test_intent_context_english() {
        let analysis = classify_query("i'm working on the recall system");
        assert_eq!(analysis.query_intent, QueryIntent::Context);
    }

    #[test]
    fn test_intent_context_chinese() {
        let analysis = classify_query("我在搞一个新的feature");
        assert_eq!(analysis.query_intent, QueryIntent::Context);
    }

    #[test]
    fn test_intent_general_fallback() {
        let analysis = classify_query("hello");
        assert_eq!(analysis.query_intent, QueryIntent::General);
    }

    // ── Type affinity tests ────────────────────────────────────────

    #[test]
    fn test_type_affinity_definition_boosts_factual() {
        let affinity = QueryIntent::Definition.type_affinity();
        assert!(affinity.factual > 1.5, "Definition should boost factual: {}", affinity.factual);
        assert!(affinity.emotional < 0.5, "Definition should suppress emotional: {}", affinity.emotional);
    }

    #[test]
    fn test_type_affinity_howto_boosts_procedural() {
        let affinity = QueryIntent::HowTo.type_affinity();
        assert!(affinity.procedural > 2.0, "HowTo should strongly boost procedural: {}", affinity.procedural);
        assert!(affinity.emotional < 0.5, "HowTo should suppress emotional: {}", affinity.emotional);
    }

    #[test]
    fn test_type_affinity_event_boosts_episodic() {
        let affinity = QueryIntent::Event.type_affinity();
        assert!(affinity.episodic > 2.0, "Event should strongly boost episodic: {}", affinity.episodic);
        assert!(affinity.emotional > 1.0, "Event should boost emotional: {}", affinity.emotional);
        assert!(affinity.procedural < 0.5, "Event should suppress procedural: {}", affinity.procedural);
    }

    #[test]
    fn test_type_affinity_relational_boosts_relational() {
        let affinity = QueryIntent::Relational.type_affinity();
        assert!(affinity.relational > 2.0, "Relational should strongly boost relational: {}", affinity.relational);
        assert!(affinity.opinion > 1.0, "Relational should boost opinion: {}", affinity.opinion);
    }

    #[test]
    fn test_type_affinity_general_is_neutral() {
        let affinity = QueryIntent::General.type_affinity();
        assert_eq!(affinity.factual, 1.0);
        assert_eq!(affinity.episodic, 1.0);
        assert_eq!(affinity.relational, 1.0);
        assert_eq!(affinity.emotional, 1.0);
        assert_eq!(affinity.procedural, 1.0);
        assert_eq!(affinity.opinion, 1.0);
        assert_eq!(affinity.causal, 1.0);
    }

    #[test]
    fn test_neutral_analysis_has_general_intent() {
        let analysis = QueryAnalysis::neutral();
        assert_eq!(analysis.query_intent, QueryIntent::General);
        assert_eq!(analysis.type_affinity.factual, 1.0);
    }

    // ── RegexSet and new pattern tests ───────────────────────────────

    #[test]
    fn test_regex_set_unicode_w() {
        // Verify that RegexSet with \w matches Chinese characters
        let rs = RegexSet::new(&["什么\\w+"]).unwrap();
        assert!(rs.is_match("什么书"));
        assert!(rs.is_match("什么东西"));
    }

    #[test]
    fn test_new_regex_howto_zh_verb() {
        // "怎么配置" should match "怎么\w+" pattern → HowTo
        assert_eq!(classify_intent_regex("怎么配置"), QueryIntent::HowTo);
    }

    #[test]
    fn test_new_regex_definition_zh_what_tool() {
        // "用了什么工具" should match "用了什么" → Definition
        assert_eq!(classify_intent_regex("用了什么工具"), QueryIntent::Definition);
    }

    #[test]
    fn test_new_regex_definition_zh_who() {
        // "谁写了这个" should match "谁" → Definition
        assert_eq!(classify_intent_regex("谁写了这个"), QueryIntent::Definition);
    }

    #[test]
    fn test_classify_query_with_l2_none_classifier() {
        // L1-only path: no Haiku classifier → regex only
        let analysis = classify_query_with_l2("what is Rust", None);
        assert_eq!(analysis.query_intent, QueryIntent::Definition);

        let analysis2 = classify_query_with_l2("hello world", None);
        assert_eq!(analysis2.query_intent, QueryIntent::General);
    }

    #[test]
    fn test_haiku_parse_intent_valid() {
        assert_eq!(HaikuIntentClassifier::parse_intent("definition"), QueryIntent::Definition);
        assert_eq!(HaikuIntentClassifier::parse_intent("howto"), QueryIntent::HowTo);
        assert_eq!(HaikuIntentClassifier::parse_intent("event"), QueryIntent::Event);
        assert_eq!(HaikuIntentClassifier::parse_intent("relational"), QueryIntent::Relational);
        assert_eq!(HaikuIntentClassifier::parse_intent("context"), QueryIntent::Context);
        assert_eq!(HaikuIntentClassifier::parse_intent("general"), QueryIntent::General);
    }

    #[test]
    fn test_haiku_parse_intent_case_insensitive() {
        assert_eq!(HaikuIntentClassifier::parse_intent("Definition"), QueryIntent::Definition);
        assert_eq!(HaikuIntentClassifier::parse_intent("HOWTO"), QueryIntent::HowTo);
        assert_eq!(HaikuIntentClassifier::parse_intent("  Event  "), QueryIntent::Event);
    }

    #[test]
    fn test_haiku_parse_intent_invalid() {
        assert_eq!(HaikuIntentClassifier::parse_intent("unknown"), QueryIntent::General);
        assert_eq!(HaikuIntentClassifier::parse_intent(""), QueryIntent::General);
        assert_eq!(HaikuIntentClassifier::parse_intent("something else entirely"), QueryIntent::General);
    }
}
