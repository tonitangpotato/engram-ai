# Context Assembly

**Tokens**: 10108/15000 | **Nodes**: 250 visited, 250 included, 0 filtered
**Elapsed**: 2ms

## Targets

### `file:src/query_classifier.rs` — query_classifier.rs
**File**: `src/query_classifier.rs`
*~17 tokens*

### `file:src/hybrid_search.rs` — hybrid_search.rs
**File**: `src/hybrid_search.rs`
*~16 tokens*

### `file:src/session_wm.rs` — session_wm.rs
**File**: `src/session_wm.rs`
*~15 tokens*

### `file:src/compiler/discovery.rs` — discovery.rs
**File**: `src/compiler/discovery.rs`
*~15 tokens*

### `file:src/compiler/compilation.rs` — compilation.rs
**File**: `src/compiler/compilation.rs`
*~16 tokens*

### `file:src/synthesis/engine.rs` — engine.rs
**File**: `src/synthesis/engine.rs`
*~14 tokens*

### `file:src/store_api.rs` — store_api.rs
**File**: `src/store_api.rs`
*~15 tokens*

## Dependencies

- **`module:src`** (`src`) — belongs_to | score: 0.52
- **`module:src/compiler`** (`src/compiler`) — belongs_to | score: 0.52
- **`module:src/synthesis`** (`src/synthesis`) — belongs_to | score: 0.52
## Callers

- **`class:src/query_classifier.rs:ChannelWeightModifiers`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `pub struct ChannelWeightModifiers`
- **`class:src/query_classifier.rs:HaikuIntentClassifier`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `pub struct HaikuIntentClassifier`
- **`class:src/query_classifier.rs:IntentRegex`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `struct IntentRegex`
- **`class:src/query_classifier.rs:NUnitAgo`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `struct NUnitAgo`
- **`class:src/query_classifier.rs:QueryAnalysis`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `pub struct QueryAnalysis`
- **`class:src/query_classifier.rs:QueryIntent`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `pub enum QueryIntent`
- **`class:src/query_classifier.rs:QueryType`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `pub enum QueryType`
- **`class:src/query_classifier.rs:TimeRange`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `pub struct TimeRange`
- **`class:src/query_classifier.rs:TypeAffinity`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `pub struct TypeAffinity`
- **`const:src/query_classifier.rs:INTENT_CONTEXT_EN`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const INTENT_CONTEXT_EN: &[&str] = &[
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

struct IntentRegex`
- **`const:src/query_classifier.rs:INTENT_CONTEXT_ZH`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const INTENT_CONTEXT_ZH: &[&str] = &[
    "我在做", "我在搞", "正在做", "正在搞",
    "手上有", "目前在", "现在在",
];

// ── RegexSet-based intent matching ─────────────────────────────────

struct IntentRegex`
- **`const:src/query_classifier.rs:INTENT_DEFINITION_EN`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const INTENT_DEFINITION_EN: &[&str] = &[
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

struct IntentRegex`
- **`const:src/query_classifier.rs:INTENT_DEFINITION_ZH`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const INTENT_DEFINITION_ZH: &[&str] = &[
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

struct IntentRegex`
- **`const:src/query_classifier.rs:INTENT_EVENT_EN`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const INTENT_EVENT_EN: &[&str] = &[
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

struct IntentRegex`
- **`const:src/query_classifier.rs:INTENT_EVENT_ZH`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const INTENT_EVENT_ZH: &[&str] = &[
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

struct IntentRegex`
- **`const:src/query_classifier.rs:INTENT_HOWTO_EN`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const INTENT_HOWTO_EN: &[&str] = &[
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

struct IntentRegex`
- **`const:src/query_classifier.rs:INTENT_HOWTO_ZH`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const INTENT_HOWTO_ZH: &[&str] = &[
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

struct IntentRegex`
- **`const:src/query_classifier.rs:INTENT_REGEX`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `static INTENT_REGEX: OnceLock<IntentRegex> = OnceLock::new();

fn get_intent_regex() -> &'static IntentRegex`
- **`const:src/query_classifier.rs:INTENT_RELATIONAL_EN`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const INTENT_RELATIONAL_EN: &[&str] = &[
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

struct IntentRegex`
- **`const:src/query_classifier.rs:INTENT_RELATIONAL_ZH`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const INTENT_RELATIONAL_ZH: &[&str] = &[
    "关系", "认识", "之间",
    "觉得怎么样", "看法", "对.*的看法",
    "怎么看",
];

const INTENT_CONTEXT_ZH: &[&str] = &[
    "我在做", "我在搞", "正在做", "正在搞",
    "手上有", "目前在", "现在在",
];

// ── RegexSet-based intent matching ─────────────────────────────────

struct IntentRegex`
- **`const:src/query_classifier.rs:TEMPORAL_EN`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const TEMPORAL_EN: &[&str] = &[
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
struct NUnitAgo`
- **`const:src/query_classifier.rs:TEMPORAL_ZH`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `const TEMPORAL_ZH: &[&str] = &[
    "昨天", "今天", "上周", "上个月", "去年",
    "这周", "这个月", "今年",
    "前天", "大前天", "刚才", "之前",
    "几天前", "几周前", "几个月前",
    "上午", "下午", "晚上", "早上",
];

/// Regex-like patterns for "N days/hours/weeks ago"
/// Result of "N units ago" detection. Unit is owned to avoid lifetime issues.
#[derive(Debug, Clone)]
struct NUnitAgo`
- **`func:src/query_classifier.rs:has_n_unit_ago`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn has_n_unit_ago(query: &str) -> Option<NUnitAgo>`
- **`func:src/query_classifier.rs:is_temporal_query`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn is_temporal_query(query: &str) -> bool`
- **`func:src/query_classifier.rs:is_keyword_query`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn is_keyword_query(query: &str) -> bool`
- **`func:src/query_classifier.rs:has_camel_case`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn has_camel_case(word: &str) -> bool`
- **`func:src/query_classifier.rs:is_semantic_query`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn is_semantic_query(query: &str) -> bool`
- **`func:src/query_classifier.rs:extract_time_range`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn extract_time_range(query: &str) -> Option<TimeRange>`
- **`func:src/query_classifier.rs:get_intent_regex`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn get_intent_regex() -> &'static IntentRegex`
- **`func:src/query_classifier.rs:classify_intent_regex`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `pub fn classify_intent_regex(query: &str) -> QueryIntent`
- **`func:src/query_classifier.rs:classify_query`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `pub fn classify_query(query: &str) -> QueryAnalysis`
- **`func:src/query_classifier.rs:classify_query_with_l2`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `pub fn classify_query_with_l2(
    query: &str,
    haiku_classifier: Option<&HaikuIntentClassifier>,
) -> QueryAnalysis`
- **`func:src/query_classifier.rs:classify_query_inner`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn classify_query_inner(query: &str, intent: QueryIntent, type_affinity: TypeAffinity) -> QueryAnalysis`
- **`func:src/query_classifier.rs:tests::test_classify_temporal_english`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_classify_temporal_english()`
- **`func:src/query_classifier.rs:tests::test_classify_temporal_chinese`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_classify_temporal_chinese()`
- **`func:src/query_classifier.rs:tests::test_classify_keyword`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_classify_keyword()`
- **`func:src/query_classifier.rs:tests::test_classify_semantic`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_classify_semantic()`
- **`func:src/query_classifier.rs:tests::test_classify_general`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_classify_general()`
- **`func:src/query_classifier.rs:tests::test_time_range_yesterday`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_time_range_yesterday()`
- **`func:src/query_classifier.rs:tests::test_time_range_last_week`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_time_range_last_week()`
- **`func:src/query_classifier.rs:tests::test_time_range_n_days_ago`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_time_range_n_days_ago()`
- **`func:src/query_classifier.rs:tests::test_weight_modifiers_temporal`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_weight_modifiers_temporal()`
- **`func:src/query_classifier.rs:tests::test_neutral_analysis`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_neutral_analysis()`
- **`func:src/query_classifier.rs:tests::test_classify_temporal_n_days_ago_chinese`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_classify_temporal_n_days_ago_chinese()`
- **`func:src/query_classifier.rs:tests::test_classify_keyword_error_code`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_classify_keyword_error_code()`
- **`func:src/query_classifier.rs:tests::test_classify_keyword_snake_case`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_classify_keyword_snake_case()`
- **`func:src/query_classifier.rs:tests::test_intent_definition_english`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_intent_definition_english()`
- **`func:src/query_classifier.rs:tests::test_intent_definition_chinese`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_intent_definition_chinese()`
- **`func:src/query_classifier.rs:tests::test_intent_howto_english`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_intent_howto_english()`
- **`func:src/query_classifier.rs:tests::test_intent_howto_chinese`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_intent_howto_chinese()`
- **`func:src/query_classifier.rs:tests::test_intent_event_english`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_intent_event_english()`
- **`func:src/query_classifier.rs:tests::test_intent_event_chinese`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_intent_event_chinese()`
- **`func:src/query_classifier.rs:tests::test_intent_relational_english`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_intent_relational_english()`
- **`func:src/query_classifier.rs:tests::test_intent_relational_chinese`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_intent_relational_chinese()`
- **`func:src/query_classifier.rs:tests::test_intent_context_english`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_intent_context_english()`
- **`func:src/query_classifier.rs:tests::test_intent_context_chinese`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_intent_context_chinese()`
- **`func:src/query_classifier.rs:tests::test_intent_general_fallback`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_intent_general_fallback()`
- **`func:src/query_classifier.rs:tests::test_type_affinity_definition_boosts_factual`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_type_affinity_definition_boosts_factual()`
- **`func:src/query_classifier.rs:tests::test_type_affinity_howto_boosts_procedural`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_type_affinity_howto_boosts_procedural()`
- **`func:src/query_classifier.rs:tests::test_type_affinity_event_boosts_episodic`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_type_affinity_event_boosts_episodic()`
- **`func:src/query_classifier.rs:tests::test_type_affinity_relational_boosts_relational`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_type_affinity_relational_boosts_relational()`
- **`func:src/query_classifier.rs:tests::test_type_affinity_general_is_neutral`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_type_affinity_general_is_neutral()`
- **`func:src/query_classifier.rs:tests::test_neutral_analysis_has_general_intent`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_neutral_analysis_has_general_intent()`
- **`func:src/query_classifier.rs:tests::test_regex_set_unicode_w`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_regex_set_unicode_w()`
- **`func:src/query_classifier.rs:tests::test_new_regex_howto_zh_verb`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_new_regex_howto_zh_verb()`
- **`func:src/query_classifier.rs:tests::test_new_regex_definition_zh_what_tool`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_new_regex_definition_zh_what_tool()`
- **`func:src/query_classifier.rs:tests::test_new_regex_definition_zh_who`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_new_regex_definition_zh_who()`
- **`func:src/query_classifier.rs:tests::test_classify_query_with_l2_none_classifier`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_classify_query_with_l2_none_classifier()`
- **`func:src/query_classifier.rs:tests::test_haiku_parse_intent_valid`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_haiku_parse_intent_valid()`
- **`func:src/query_classifier.rs:tests::test_haiku_parse_intent_case_insensitive`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_haiku_parse_intent_case_insensitive()`
- **`func:src/query_classifier.rs:tests::test_haiku_parse_intent_invalid`** (`src/query_classifier.rs`) — defined_in | score: 0.76
  Sig: `fn test_haiku_parse_intent_invalid()`
- **`infer:component:0.6`** — contains | score: 0.76
  Groups the public API interface and its associated test suites for knowledge compilation.
- **`infer:component:0.28.0`** — contains | score: 0.76
  Anthropic API client and query classification logic for LLM interactions.
- **`class:src/hybrid_search.rs:HybridSearchOpts`** (`src/hybrid_search.rs`) — defined_in | score: 0.76
  Sig: `pub struct HybridSearchOpts`
- **`class:src/hybrid_search.rs:HybridSearchResult`** (`src/hybrid_search.rs`) — defined_in | score: 0.76
  Sig: `pub struct HybridSearchResult`
- **`func:src/hybrid_search.rs:hybrid_search`** (`src/hybrid_search.rs`) — defined_in | score: 0.76
  Sig: `pub fn hybrid_search(
    storage: &Storage,
    query_vector: Option<&[f32]>,
    query_text: &str,
    opts: HybridSearchOpts,
    model: &str,
) -> Result<Vec<HybridSearchResult>, Box<dyn std::error::Error>>`
- **`func:src/hybrid_search.rs:adaptive_hybrid_search`** (`src/hybrid_search.rs`) — defined_in | score: 0.76
  Sig: `pub fn adaptive_hybrid_search(
    storage: &Storage,
    query_vector: Option<&[f32]>,
    query_text: &str,
    limit: usize,
    model: &str,
) -> Result<Vec<HybridSearchResult>, Box<dyn std::error::Error>>`
- **`func:src/hybrid_search.rs:reciprocal_rank_fusion`** (`src/hybrid_search.rs`) — defined_in | score: 0.76
  Sig: `pub fn reciprocal_rank_fusion(
    storage: &Storage,
    query_vector: Option<&[f32]>,
    query_text: &str,
    limit: usize,
    k: f64, // RRF constant, typically 60
    model: &str,
) -> Result<Vec<HybridSearchResult>, Box<dyn std::error::Error>>`
- **`func:src/hybrid_search.rs:jaccard_similarity`** (`src/hybrid_search.rs`) — defined_in | score: 0.76
  Sig: `pub fn jaccard_similarity(set_a: &HashSet<String>, set_b: &HashSet<String>) -> f64`
- **`func:src/hybrid_search.rs:tests::test_jaccard_similarity`** (`src/hybrid_search.rs`) — defined_in | score: 0.76
  Sig: `fn test_jaccard_similarity()`
- **`func:src/hybrid_search.rs:tests::test_jaccard_identical`** (`src/hybrid_search.rs`) — defined_in | score: 0.76
  Sig: `fn test_jaccard_identical()`
- **`func:src/hybrid_search.rs:tests::test_jaccard_disjoint`** (`src/hybrid_search.rs`) — defined_in | score: 0.76
  Sig: `fn test_jaccard_disjoint()`
- **`func:src/hybrid_search.rs:tests::test_jaccard_empty`** (`src/hybrid_search.rs`) — defined_in | score: 0.76
  Sig: `fn test_jaccard_empty()`
- **`func:src/hybrid_search.rs:tests::test_hybrid_search_opts_default`** (`src/hybrid_search.rs`) — defined_in | score: 0.76
  Sig: `fn test_hybrid_search_opts_default()`
- **`infer:component:0.12`** — contains | score: 0.76
  Implements hybrid search functionality combining multiple search strategies.
- **`class:src/session_wm.rs:CachedScore`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `pub struct CachedScore`
- **`class:src/session_wm.rs:SessionRecallResult`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `pub struct SessionRecallResult`
- **`class:src/session_wm.rs:SessionRegistry`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `pub struct SessionRegistry`
- **`class:src/session_wm.rs:SessionWorkingMemory`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `pub struct SessionWorkingMemory`
- **`const:src/session_wm.rs:DEFAULT_CAPACITY`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `const DEFAULT_CAPACITY: usize = 7;

/// Default decay time for working memory items (5 minutes).
const DEFAULT_DECAY_SECS: u64 = 300;

/// Scores cached from the full recall that populated a working memory item.
#[derive(Debug, Clone)]
pub struct CachedScore`
- **`const:src/session_wm.rs:DEFAULT_DECAY_SECS`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `const DEFAULT_DECAY_SECS: u64 = 300;

/// Scores cached from the full recall that populated a working memory item.
#[derive(Debug, Clone)]
pub struct CachedScore`
- **`func:src/session_wm.rs:tests::test_basic_activation`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `fn test_basic_activation()`
- **`func:src/session_wm.rs:tests::test_capacity_pruning`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `fn test_capacity_pruning()`
- **`func:src/session_wm.rs:tests::test_overlap_calculation`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `fn test_overlap_calculation()`
- **`func:src/session_wm.rs:tests::test_topic_continuity`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `fn test_topic_continuity()`
- **`func:src/session_wm.rs:tests::test_session_registry`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `fn test_session_registry()`
- **`func:src/session_wm.rs:tests::test_decay_pruning`** (`src/session_wm.rs`) — defined_in | score: 0.76
  Sig: `fn test_decay_pruning()`
- **`infer:component:infrastructure`** — contains | score: 0.76
  Core platform infrastructure including compiler, storage, discovery, embeddings, and event bus systems.
- **`class:src/compiler/discovery.rs:CosineMetric`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `struct CosineMetric;

impl Metric<Vec<f32>> for CosineMetric`
- **`class:src/compiler/discovery.rs:TopicDiscovery`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `pub struct TopicDiscovery`
- **`class:src/compiler/discovery.rs:tests::MockLlmProvider`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `struct MockLlmProvider`
- **`const:src/compiler/discovery.rs:ANN_THRESHOLD`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `const ANN_THRESHOLD: usize = 100;

/// Default number of nearest neighbors to find per node.
const DEFAULT_TOP_K: usize = 20;

// HNSW parameters
/// M: max connections per node at layers > 0
const HNSW_M: usize = 24;
/// M0: max connections per node at layer 0
const HNSW_M0: usize = 48;
/// ef_construction: candidate pool size during build
const HNSW_EF_CONSTRUCTION: usize = 100;
/// ef_search: candidate pool size during search
const HNSW_EF_SEARCH: usize = 50;

// ═══════════════════════════════════════════════════════════════════════════════
//  TOPIC DISCOVERY
// ═══════════════════════════════════════════════════════════════════════════════

/// Discovers topic candidates from a set of memories using Infomap community
/// detection on a cosine-similarity graph.
pub struct TopicDiscovery`
- **`const:src/compiler/discovery.rs:DEFAULT_TOP_K`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `const DEFAULT_TOP_K: usize = 20;

// HNSW parameters
/// M: max connections per node at layers > 0
const HNSW_M: usize = 24;
/// M0: max connections per node at layer 0
const HNSW_M0: usize = 48;
/// ef_construction: candidate pool size during build
const HNSW_EF_CONSTRUCTION: usize = 100;
/// ef_search: candidate pool size during search
const HNSW_EF_SEARCH: usize = 50;

// ═══════════════════════════════════════════════════════════════════════════════
//  TOPIC DISCOVERY
// ═══════════════════════════════════════════════════════════════════════════════

/// Discovers topic candidates from a set of memories using Infomap community
/// detection on a cosine-similarity graph.
pub struct TopicDiscovery`
- **`const:src/compiler/discovery.rs:HNSW_EF_CONSTRUCTION`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `const HNSW_EF_CONSTRUCTION: usize = 100;
/// ef_search: candidate pool size during search
const HNSW_EF_SEARCH: usize = 50;

// ═══════════════════════════════════════════════════════════════════════════════
//  TOPIC DISCOVERY
// ═══════════════════════════════════════════════════════════════════════════════

/// Discovers topic candidates from a set of memories using Infomap community
/// detection on a cosine-similarity graph.
pub struct TopicDiscovery`
- **`const:src/compiler/discovery.rs:HNSW_EF_SEARCH`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `const HNSW_EF_SEARCH: usize = 50;

// ═══════════════════════════════════════════════════════════════════════════════
//  TOPIC DISCOVERY
// ═══════════════════════════════════════════════════════════════════════════════

/// Discovers topic candidates from a set of memories using Infomap community
/// detection on a cosine-similarity graph.
pub struct TopicDiscovery`
- **`const:src/compiler/discovery.rs:HNSW_M`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `const HNSW_M: usize = 24;
/// M0: max connections per node at layer 0
const HNSW_M0: usize = 48;
/// ef_construction: candidate pool size during build
const HNSW_EF_CONSTRUCTION: usize = 100;
/// ef_search: candidate pool size during search
const HNSW_EF_SEARCH: usize = 50;

// ═══════════════════════════════════════════════════════════════════════════════
//  TOPIC DISCOVERY
// ═══════════════════════════════════════════════════════════════════════════════

/// Discovers topic candidates from a set of memories using Infomap community
/// detection on a cosine-similarity graph.
pub struct TopicDiscovery`
- **`const:src/compiler/discovery.rs:HNSW_M0`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `const HNSW_M0: usize = 48;
/// ef_construction: candidate pool size during build
const HNSW_EF_CONSTRUCTION: usize = 100;
/// ef_search: candidate pool size during search
const HNSW_EF_SEARCH: usize = 50;

// ═══════════════════════════════════════════════════════════════════════════════
//  TOPIC DISCOVERY
// ═══════════════════════════════════════════════════════════════════════════════

/// Discovers topic candidates from a set of memories using Infomap community
/// detection on a cosine-similarity graph.
pub struct TopicDiscovery`
- **`func:src/compiler/discovery.rs:tests::make_topic_page`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn make_topic_page(id: &str, source_ids: Vec<&str>) -> TopicPage`
- **`func:src/compiler/discovery.rs:tests::test_discover_basic_two_clusters`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_basic_two_clusters()`
- **`func:src/compiler/discovery.rs:tests::test_discover_empty`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_empty()`
- **`func:src/compiler/discovery.rs:tests::test_discover_single_memory`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_single_memory()`
- **`func:src/compiler/discovery.rs:tests::test_discover_min_cluster_size`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_min_cluster_size()`
- **`func:src/compiler/discovery.rs:tests::test_edge_threshold_controls_granularity`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_edge_threshold_controls_granularity()`
- **`func:src/compiler/discovery.rs:tests::test_discover_cohesion_score`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_cohesion_score()`
- **`func:src/compiler/discovery.rs:tests::test_discover_no_chaining_effect`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_no_chaining_effect()`
- **`func:src/compiler/discovery.rs:tests::test_label_cluster_success`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_label_cluster_success()`
- **`func:src/compiler/discovery.rs:tests::test_label_cluster_fallback`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_label_cluster_fallback()`
- **`func:src/compiler/discovery.rs:tests::test_detect_overlap`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_detect_overlap()`
- **`func:src/compiler/discovery.rs:tests::test_detect_no_overlap`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_detect_no_overlap()`
- **`func:src/compiler/discovery.rs:tests::test_top_k_builder`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_top_k_builder()`
- **`func:src/compiler/discovery.rs:tests::test_discover_ann_with_many_memories`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_ann_with_many_memories()`
- **`func:src/compiler/discovery.rs:tests::test_discover_two_memories`** (`src/compiler/discovery.rs`) — defined_in | score: 0.76
  Sig: `fn test_discover_two_memories()`
- **`infer:component:0.2`** — contains | score: 0.76
  Main association module interface and public API definitions.
- **`class:src/compiler/compilation.rs:ChangeDetector`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub struct ChangeDetector;

impl ChangeDetector`
- **`class:src/compiler/compilation.rs:CompilationPipeline`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub struct CompilationPipeline<S: KnowledgeStore, L: LlmProvider>`
- **`class:src/compiler/compilation.rs:MemorySnapshot`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub struct MemorySnapshot`
- **`class:src/compiler/compilation.rs:QualityScorer`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub struct QualityScorer<'a>`
- **`class:src/compiler/compilation.rs:TriggerEvaluator`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub struct TriggerEvaluator<'a>`
- **`func:src/compiler/compilation.rs:render_memory_line`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn render_memory_line(m: &MemorySnapshot, level: PromptDetailLevel) -> String`
- **`func:src/compiler/compilation.rs:estimate_tokens`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn estimate_tokens(s: &str, tokens_per_char: f64) -> usize`
- **`func:src/compiler/compilation.rs:enforce_prompt_budget`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn enforce_prompt_budget<'m>(
    memories: &'m [MemorySnapshot],
    cfg: &PromptConfig,
    header_tokens: usize,
) -> (Vec<&'m MemorySnapshot>, usize)`
- **`func:src/compiler/compilation.rs:build_full_compile_prompt`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub fn build_full_compile_prompt(
    title: &str,
    memories: &[MemorySnapshot],
    user_edits: &[(String, String)],
) -> String`
- **`func:src/compiler/compilation.rs:build_full_compile_prompt_with`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub fn build_full_compile_prompt_with(
    title: &str,
    memories: &[MemorySnapshot],
    user_edits: &[(String, String)],
    cfg: &PromptConfig,
) -> String`
- **`func:src/compiler/compilation.rs:build_incremental_compile_prompt`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub fn build_incremental_compile_prompt(
    title: &str,
    existing_content: &str,
    changes: &ChangeSet,
    memories: &[MemorySnapshot],
    user_edits: &[(String, String)],
) -> String`
- **`func:src/compiler/compilation.rs:build_incremental_compile_prompt_with`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub fn build_incremental_compile_prompt_with(
    title: &str,
    existing_content: &str,
    changes: &ChangeSet,
    memories: &[MemorySnapshot],
    user_edits: &[(String, String)],
    cfg: &PromptConfig,
) -> String`
- **`func:src/compiler/compilation.rs:compile_without_llm`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub fn compile_without_llm(title: &str, memories: &[MemorySnapshot]) -> String`
- **`func:src/compiler/compilation.rs:preserve_user_edits`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub fn preserve_user_edits(content: &str, edits: &[(String, String)]) -> String`
- **`func:src/compiler/compilation.rs:simple_hash_embedding`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub fn simple_hash_embedding(content: &str, dims: usize) -> Vec<f32>`
- **`func:src/compiler/compilation.rs:extract_summary`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub fn extract_summary(content: &str) -> String`
- **`func:src/compiler/compilation.rs:aggregate_tags`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `pub fn aggregate_tags(memories: &[MemorySnapshot]) -> Vec<String>`
- **`func:src/compiler/compilation.rs:tests::make_config`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn make_config() -> KcConfig`
- **`func:src/compiler/compilation.rs:tests::make_topic`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn make_topic(id: &str, compilation_count: u32, quality: Option<f64>) -> TopicPage`
- **`func:src/compiler/compilation.rs:tests::test_detect_first_compilation`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_detect_first_compilation()`
- **`func:src/compiler/compilation.rs:tests::test_detect_with_changes`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_detect_with_changes()`
- **`func:src/compiler/compilation.rs:tests::test_trigger_skip_no_changes`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_trigger_skip_no_changes()`
- **`func:src/compiler/compilation.rs:tests::test_trigger_initial_compilation`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_trigger_initial_compilation()`
- **`func:src/compiler/compilation.rs:tests::test_trigger_eager_full_recompile`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_trigger_eager_full_recompile()`
- **`func:src/compiler/compilation.rs:tests::test_trigger_eager_partial_recompile`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_trigger_eager_partial_recompile()`
- **`func:src/compiler/compilation.rs:tests::test_trigger_manual_always_skips`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_trigger_manual_always_skips()`
- **`func:src/compiler/compilation.rs:tests::test_quality_scorer_good`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_quality_scorer_good()`
- **`func:src/compiler/compilation.rs:tests::test_quality_scorer_poor_coverage`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_quality_scorer_poor_coverage()`
- **`func:src/compiler/compilation.rs:tests::test_quality_scorer_short_content`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_quality_scorer_short_content()`
- **`func:src/compiler/compilation.rs:tests::test_compile_without_llm`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_compile_without_llm()`
- **`func:src/compiler/compilation.rs:tests::test_preserve_user_edits_found`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_preserve_user_edits_found()`
- **`func:src/compiler/compilation.rs:tests::test_preserve_user_edits_not_found`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_preserve_user_edits_not_found()`
- **`func:src/compiler/compilation.rs:tests::test_full_compile_prompt_structure`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_full_compile_prompt_structure()`
- **`func:src/compiler/compilation.rs:tests::test_incremental_compile_prompt`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_incremental_compile_prompt()`
- **`func:src/compiler/compilation.rs:tests::test_dry_run_no_existing_topics_all_new`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_dry_run_no_existing_topics_all_new()`
- **`func:src/compiler/compilation.rs:tests::test_dry_run_existing_topics_no_changes_skip`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_dry_run_existing_topics_no_changes_skip()`
- **`func:src/compiler/compilation.rs:tests::test_verbose_compilation_succeeds`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn test_verbose_compilation_succeeds()`
- **`func:src/compiler/compilation.rs:tests::make_dims`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn make_dims(stance: Option<&str>, cause: Option<&str>, domain: Option<&str>) -> Dimensions`
- **`func:src/compiler/compilation.rs:tests::iss020_p0_4_legacy_memory_uses_minimal_format_even_when_enriched_requested`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn iss020_p0_4_legacy_memory_uses_minimal_format_even_when_enriched_requested()`
- **`func:src/compiler/compilation.rs:tests::iss020_p0_4_enriched_memory_surfaces_dimensional_pipes`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn iss020_p0_4_enriched_memory_surfaces_dimensional_pipes()`
- **`func:src/compiler/compilation.rs:tests::iss020_p0_4_minimal_level_reproduces_legacy_exactly`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn iss020_p0_4_minimal_level_reproduces_legacy_exactly()`
- **`func:src/compiler/compilation.rs:tests::iss020_p0_4_token_budget_drops_lowest_importance`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn iss020_p0_4_token_budget_drops_lowest_importance()`
- **`func:src/compiler/compilation.rs:tests::iss020_p0_4_partial_dimensions_only_emit_populated_pipes`** (`src/compiler/compilation.rs`) — defined_in | score: 0.76
  Sig: `fn iss020_p0_4_partial_dimensions_only_emit_populated_pipes()`
- **`class:src/synthesis/engine.rs:DefaultSynthesisEngine`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `pub struct DefaultSynthesisEngine`
- **`class:src/synthesis/engine.rs:tests::FailingLlmProvider`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `struct FailingLlmProvider;

    impl SynthesisLlmProvider for FailingLlmProvider`
- **`class:src/synthesis/engine.rs:tests::MockLlmProvider`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `struct MockLlmProvider`
- **`func:src/synthesis/engine.rs:linear_index_to_pair`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn linear_index_to_pair(idx: usize, n: usize) -> (usize, usize)`
- **`func:src/synthesis/engine.rs:generate_id`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn generate_id() -> String`
- **`func:src/synthesis/engine.rs:tests::make_memory`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn make_memory(id: &str, content: &str, memory_type: MemoryType, importance: f64) -> MemoryRecord`
- **`func:src/synthesis/engine.rs:tests::setup_storage_with_memories`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn setup_storage_with_memories(memories: &[MemoryRecord]) -> Storage`
- **`func:src/synthesis/engine.rs:tests::default_settings`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn default_settings() -> SynthesisSettings`
- **`func:src/synthesis/engine.rs:tests::make_cluster`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn make_cluster(id: &str, members: &[&str], quality: f64) -> MemoryCluster`
- **`func:src/synthesis/engine.rs:tests::test_should_resynthesize_new_cluster`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_should_resynthesize_new_cluster()`
- **`func:src/synthesis/engine.rs:tests::test_should_resynthesize_no_change`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_should_resynthesize_no_change()`
- **`func:src/synthesis/engine.rs:tests::test_should_resynthesize_member_change`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_should_resynthesize_member_change()`
- **`func:src/synthesis/engine.rs:tests::test_should_resynthesize_quality_delta`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_should_resynthesize_quality_delta()`
- **`func:src/synthesis/engine.rs:tests::test_incremental_state_storage_roundtrip`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_incremental_state_storage_roundtrip()`
- **`func:src/synthesis/engine.rs:tests::test_incremental_state_missing`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_incremental_state_missing()`
- **`func:src/synthesis/engine.rs:tests::test_synthesize_skips_unchanged_clusters`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_synthesize_skips_unchanged_clusters()`
- **`func:src/synthesis/engine.rs:tests::test_cluster_changed_no_prior_attempt`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_cluster_changed_no_prior_attempt()`
- **`func:src/synthesis/engine.rs:tests::test_cluster_changed_same_members_returns_false`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_cluster_changed_same_members_returns_false()`
- **`func:src/synthesis/engine.rs:tests::test_attempt_count_persisted`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_attempt_count_persisted()`
- **`func:src/synthesis/engine.rs:tests::test_cluster_changed_jaccard_detects_membership_change`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_cluster_changed_jaccard_detects_membership_change()`
- **`func:src/synthesis/engine.rs:tests::test_cluster_changed_jaccard_identical_members`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_cluster_changed_jaccard_identical_members()`
- **`func:src/synthesis/engine.rs:tests::test_linear_index_to_pair`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_linear_index_to_pair()`
- **`func:src/synthesis/engine.rs:tests::test_all_pairs_similar_identical_embeddings`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_all_pairs_similar_identical_embeddings()`
- **`func:src/synthesis/engine.rs:tests::test_all_pairs_similar_dissimilar_pair`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_all_pairs_similar_dissimilar_pair()`
- **`func:src/synthesis/engine.rs:tests::test_all_pairs_similar_missing_embedding`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_all_pairs_similar_missing_embedding()`
- **`func:src/synthesis/engine.rs:tests::test_all_pairs_similar_single_member`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_all_pairs_similar_single_member()`
- **`func:src/synthesis/engine.rs:tests::test_all_pairs_similar_large_cluster_sampling`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_all_pairs_similar_large_cluster_sampling()`
- **`func:src/synthesis/engine.rs:tests::test_all_pairs_similar_large_cluster_with_outlier`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_all_pairs_similar_large_cluster_with_outlier()`
- **`func:src/synthesis/engine.rs:tests::test_no_llm_provider_graceful_degradation`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_no_llm_provider_graceful_degradation()`
- **`func:src/synthesis/engine.rs:tests::test_no_llm_with_memories_skips_synthesis`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_no_llm_with_memories_skips_synthesis()`
- **`func:src/synthesis/engine.rs:tests::test_mock_llm_synthesis`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_mock_llm_synthesis()`
- **`func:src/synthesis/engine.rs:tests::test_budget_exhaustion`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_budget_exhaustion()`
- **`func:src/synthesis/engine.rs:tests::test_store_insight_atomically`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_store_insight_atomically()`
- **`func:src/synthesis/engine.rs:tests::test_generate_id_format`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_generate_id_format()`
- **`func:src/synthesis/engine.rs:tests::test_check_gate_delegation`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_check_gate_delegation()`
- **`func:src/synthesis/engine.rs:tests::test_get_provenance_delegation`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_get_provenance_delegation()`
- **`func:src/synthesis/engine.rs:tests::test_empty_storage_no_clusters`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_empty_storage_no_clusters()`
- **`func:src/synthesis/engine.rs:tests::test_cold_path_on_empty_storage`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_cold_path_on_empty_storage()`
- **`func:src/synthesis/engine.rs:tests::test_count_memories`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_count_memories()`
- **`func:src/synthesis/engine.rs:tests::test_get_all_cluster_data_empty`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_get_all_cluster_data_empty()`
- **`func:src/synthesis/engine.rs:tests::test_get_all_cluster_data_after_save`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_get_all_cluster_data_after_save()`
- **`func:src/synthesis/engine.rs:tests::test_cold_path_saves_cluster_state`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_cold_path_saves_cluster_state()`
- **`func:src/synthesis/engine.rs:tests::test_three_tier_config_defaults`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_three_tier_config_defaults()`
- **`func:src/synthesis/engine.rs:tests::test_three_tier_config_custom`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_three_tier_config_custom()`
- **`func:src/synthesis/engine.rs:tests::test_warm_path_with_pending`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_warm_path_with_pending()`
- **`func:src/synthesis/engine.rs:tests::test_cold_path_triggered_by_ratio`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_cold_path_triggered_by_ratio()`
- **`func:src/synthesis/engine.rs:tests::test_cached_path_no_pending_no_dirty`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_cached_path_no_pending_no_dirty()`
- **`func:src/synthesis/engine.rs:tests::test_emotional_modulation_wired_in_synthesis`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_emotional_modulation_wired_in_synthesis()`
- **`func:src/synthesis/engine.rs:tests::test_emotional_modulation_disabled_noop`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_emotional_modulation_disabled_noop()`
- **`func:src/synthesis/engine.rs:tests::test_emotional_boost_increases_quality`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_emotional_boost_increases_quality()`
- **`func:src/synthesis/engine.rs:tests::test_auto_update_merge_duplicates_supersedes_memories`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_auto_update_merge_duplicates_supersedes_memories()`
- **`func:src/synthesis/engine.rs:tests::test_auto_update_merge_duplicates_transfers_hebbian_links`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_auto_update_merge_duplicates_transfers_hebbian_links()`
- **`func:src/synthesis/engine.rs:tests::test_auto_update_merge_duplicates_boosts_importance`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_auto_update_merge_duplicates_boosts_importance()`
- **`func:src/synthesis/engine.rs:tests::test_auto_update_strengthen_links`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_auto_update_strengthen_links()`
- **`func:src/synthesis/engine.rs:tests::test_auto_update_reports_cluster_count`** (`src/synthesis/engine.rs`) — defined_in | score: 0.76
  Sig: `fn test_auto_update_reports_cluster_count()`
- **`infer:component:0.4`** — contains | score: 0.76
  Message bus I/O operations and corresponding test suite.
- **`infer:component:0.20.0`** — contains | score: 0.76
  Core synthesis engine with clustering capabilities and performance benchmarks.
- **`class:src/store_api.rs:ContentHash`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `pub struct ContentHash(pub String);

impl ContentHash`
- **`class:src/store_api.rs:MemoryId`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `pub type MemoryId = String;

/// Quarantine row id. Newtype so it cannot be confused with `MemoryId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuarantineId(pub String);

impl QuarantineId`
- **`class:src/store_api.rs:QuarantineId`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `pub struct QuarantineId(pub String);

impl QuarantineId`
- **`class:src/store_api.rs:QuarantineReason`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `pub enum QuarantineReason`
- **`class:src/store_api.rs:RawStoreOutcome`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `pub enum RawStoreOutcome`
- **`class:src/store_api.rs:RetryReport`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `pub struct RetryReport`
- **`class:src/store_api.rs:SkipReason`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `pub enum SkipReason`
- **`class:src/store_api.rs:StorageMeta`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `pub struct StorageMeta`
- **`class:src/store_api.rs:StoreError`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `pub enum StoreError`
- **`class:src/store_api.rs:StoreOutcome`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `pub enum StoreOutcome`
- **`func:src/store_api.rs:tests::store_outcome_id_accessor`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `fn store_outcome_id_accessor()`
- **`func:src/store_api.rs:tests::storage_meta_default`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `fn storage_meta_default()`
- **`func:src/store_api.rs:tests::quarantine_id_roundtrip`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `fn quarantine_id_roundtrip()`
- **`func:src/store_api.rs:tests::skip_reason_serde`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `fn skip_reason_serde()`
- **`func:src/store_api.rs:tests::quarantine_reason_serde_roundtrip`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `fn quarantine_reason_serde_roundtrip()`
- **`func:src/store_api.rs:tests::raw_store_outcome_stored_variant_serde`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `fn raw_store_outcome_stored_variant_serde()`
- **`func:src/store_api.rs:tests::store_error_from_rusqlite_compiles`** (`src/store_api.rs`) — defined_in | score: 0.76
  Sig: `fn store_error_from_rusqlite_compiles()`

context: 250 visited, 250 included, 0 filtered, 10108/15000 tokens, 2ms
