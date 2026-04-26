# ISS-018: Recall Intent Classification — Design

**Date**: 2026-04-21
**Status**: Ready for Implementation
**Traces to**: ISS-018-recall-intent-classification.md (investigation)

---

## 1. Overview

### Problem
engram stores memories with type labels (Factual/Episodic/Procedural/etc.) via Haiku LLM extraction, but recall completely ignores these labels. The 7-channel fusion scoring treats all memory types equally regardless of query intent, causing frequent-access episodic memories to dominate over definitional factual memories via ACT-R frequency bias.

### Solution
Two-level intent classification: L1 Regex (µs, 78→90% accuracy) + L2 Haiku API fallback (200ms, ~90%+ accuracy). The classified intent drives type-affinity multipliers already integrated into the scoring pipeline. L2 is sequential in v1 (~200ms added to ~12% of queries); parallel execution is a future optimization if p95 latency exceeds 500ms.

### Architecture Change
Replace `IntentAnchors` (embedding-based L2, 20% accuracy) with `HaikuIntentClassifier` (API-based L2, ~90% accuracy).

**Prerequisite refactor**: Extract a shared `anthropic_client` module (`src/anthropic_client.rs`) containing:
- `pub fn build_anthropic_headers(token: &str, is_oauth: bool) -> HeaderMap` — shared header construction (OAuth/API-key, anthropic-beta, user-agent spoofing)
- `pub struct StaticToken(pub String)` implementing `TokenProvider` — single canonical definition
- `pub const DEFAULT_ANTHROPIC_API_URL: &str = "https://api.anthropic.com"` — centralized API URL constant (used by `extractor.rs`, `triple_extractor.rs`, and `HaikuIntentClassifier`)

**Dependency direction**: `TokenProvider` trait stays in `extractor.rs` (where it is currently defined and publicly exported). The shared `anthropic_client.rs` module imports it via `use crate::extractor::TokenProvider`. This avoids breaking the existing public API (`pub use extractor::TokenProvider` in `lib.rs`). The shared module depends on `extractor`, not the other way around.

Currently `build_headers()` is duplicated in `extractor.rs` and `triple_extractor.rs`, and `StaticToken` is defined independently in both. Adding a third copy in `HaikuIntentClassifier` is unacceptable. This refactor consolidates all three call sites, then `HaikuIntentClassifier`, `AnthropicExtractor`, and `AnthropicTripleExtractor` all import from the shared module.

**Note on auth wiring**: The `MemoryExtractor` trait exposes only `fn extract(...)` — there is no way to reach inside the trait object to access `TokenProvider`, `client`, or `api_url` from `AnthropicExtractor`. The `HaikuIntentClassifier` must be constructed with its **own** `TokenProvider` instance and `reqwest::blocking::Client`, following the same pattern as `triple_extractor.rs`. It does NOT share the extractor's client — it uses the same auth config to construct an independent client.

---

## 2. Components

### 2.1 Regex Pattern Enhancement (`query_classifier.rs`)

**What changes**: Expand patterns to fix the 4 known misclassifications.

**Important**: The existing matching code uses `query.contains(p)` (literal string matching), NOT regex. Patterns like `"对.*的看法"` or `怎么\w+` will never match via `contains()`. All new patterns must be **literal string prefixes/substrings** that work with `contains()`, OR the matching code must be changed to use `regex::Regex`.

**Decision**: Switch to `regex::Regex` for pattern matching. The existing `contains()`-based patterns are simple enough to work as regex too (literal strings are valid regex). This enables proper wildcard patterns like `怎么\w+` and fixes the pre-existing bug where `"对.*的看法"` in `INTENT_RELATIONAL_ZH` never matches.

**Implementation**: Compile one `regex::RegexSet` per intent category at startup (once). Check categories in priority order: HowTo → Event → Definition → Relational → Context. Replace `intent_patterns.iter().any(|p| query.contains(p))` with per-category `regex_set.is_match(query)`. This preserves the existing priority ordering where earlier categories win.

```rust
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
```

Priority check order:
```rust
if self.howto_en.is_match(q) || self.howto_zh.is_match(q) { HowTo }
else if self.event_en.is_match(q) || self.event_zh.is_match(q) { Event }
else if self.definition_en.is_match(q) || self.definition_zh.is_match(q) { Definition }
else if self.relational_en.is_match(q) || self.relational_zh.is_match(q) { Relational }
else if self.context_en.is_match(q) || self.context_zh.is_match(q) { Context }
else { General }
```

New/fixed patterns:
- `"什么书|什么东西|什么人"` → Definition ZH
- `"怎么\w+"` (怎么 + any verb) → HowTo ZH  
- `"(?i)i am (?:building|working)"` → Context EN
- `"用了什么|用的什么|用什么"` → Definition ZH
- `"谁"` (who) → Definition ZH
- `"在哪|哪里"` (where) → Definition ZH
- Fix `"对.*的看法"` → now works as actual regex in Relational ZH
- Short Chinese question patterns: `什么\w+` without existing prefix

**Unicode `\w` verification**: The `regex` crate defaults to Unicode-aware `\w` (matches CJK characters), but this must be verified specifically for `RegexSet`. During implementation, add a unit test that `RegexSet::new(&["什么\\w+"])` matches "什么书". If `\w` does not match CJK in `RegexSet`, fall back to `什么[\\p{Han}\\w]+`.

**Target**: 78% → ~88% regex accuracy. Reduces L2 trigger rate from ~22% to ~12%.

### 2.2 Haiku Intent Classifier (`query_classifier.rs`)

**New struct**: `HaikuIntentClassifier`

Replaces `IntentAnchors`. Uses the shared `anthropic_client` module for header construction and `TokenProvider`. Constructed with its **own** `reqwest::blocking::Client` and `TokenProvider` instance (not shared with the extractor).

```rust
pub struct HaikuIntentClassifier {
    client: reqwest::blocking::Client,
    token_provider: Box<dyn TokenProvider>,  // from anthropic_client module
    is_oauth: bool,
    model: String,      // default: "claude-haiku-4-5-20251001"
    api_url: String,    // default: "https://api.anthropic.com"
    timeout_secs: u64,  // default: 5 (aggressive — this is L2 fallback)
    disabled: AtomicBool,  // set to true on auth failure, disables L2 for session
}
```

**New function signature** (replaces `classify_query_with_embedding`):

```rust
pub fn classify_query_with_l2(
    query: &str,
    haiku_classifier: Option<&HaikuIntentClassifier>,
) -> QueryAnalysis
```

The embedding parameter is dropped — Haiku uses text, not embeddings. Callers in `memory.rs` update accordingly (no longer pass `query_embedding` to classification).

**Prompt** (~150 input tokens, ~10 output tokens):
```
Classify the intent of this query into EXACTLY ONE category.
Categories: definition, howto, event, relational, context, general

Rules:
- definition: asking what something IS, facts, descriptions
- howto: asking HOW to do something, steps, instructions  
- event: asking what HAPPENED, when, history, timeline
- relational: asking about relationships, connections, opinions about
- context: stating what the speaker is working on (not a question)
- general: greetings, unclear, or doesn't fit above

The query may be in English or Chinese — classify based on meaning, not language.

Respond with ONLY the category name, nothing else.

Query: "{query}"
```

Example bilingual queries the prompt must handle:
- `"Tim读了什么书"` → definition
- `"怎么重启服务"` → howto
- `"What happened yesterday?"` → event

**Cost estimate**: With full message framing (model, messages array, etc.), actual input tokens are ~200-250, not the ~160 naively estimated from prompt text. Conservative estimate: 250 input + 10 output = ~$0.00007/call. Still negligible. Exact token counts should be validated with one real Haiku call during implementation.

**Parsing**: Trim whitespace, lowercase, match against known labels. If parsing fails → return `General` (safe fallback).

**Disabled state**: The `disabled: AtomicBool` field is checked before each L2 call. On auth failure (HTTP 401/403), it is set to `true`, disabling L2 for the remaining session. Checked via `classifier.disabled.load(Ordering::Relaxed)`. v1 has no auto-recovery; a future version could retry after a cooldown period.

### 2.3 Sequential Execution with Parallel Upgrade Path (`memory.rs`)

**Key constraint**: The L2 Haiku call only fires when L1 Regex returns `General` (~12% of queries).

**Current flow** (sync):
```
query → classify_query() → embed(query) → FTS search → score → return
         ↑ regex only       ↑ sequential
```

**New flow (v1 — sequential)**:
```
query → classify_intent_regex()
         ├─ if NOT General → use regex result, proceed normally
         └─ if General → Haiku intent classify (~200ms, blocking)
              → then embed(query) → scoring
```

**Latency impact (honest assessment)**: Sequential L2 adds ~200ms to ~12% of queries. The investigation's key argument for Haiku was "延迟被 recall embedding 计算掩盖（并行）" — but v1 does NOT implement parallelism, so this latency is **additive**, not hidden. Since engram recall typically happens before the agent's main LLM call (~1-3s), the 200ms is acceptable but not invisible.

**Parallel upgrade path**: If p95 recall latency exceeds 500ms, implement parallel execution:
```
query → classify_intent_regex()
         └─ if General → spawn two parallel tasks:
              ├─ task A: Haiku intent classify (blocking, in thread)
              └─ task B: embed(query)
              → join both → apply Haiku intent → proceed with scoring
```
Use `std::thread::scope` or `rayon::scope` to parallelize. The Haiku call uses `reqwest::blocking::Client`, so no async runtime needed.

**Decision**: Start with sequential (simpler, lower implementation risk). Concrete trigger for parallel: p95 recall latency > 500ms as measured in production benchmarks.

### 2.4 Configuration (`config.rs`)

Add intent classification config to `MemoryConfig`:

```rust
pub struct IntentClassificationConfig {
    /// Enable L2 Haiku fallback (default: false — requires explicit opt-in)
    pub haiku_l2_enabled: bool,
    /// Model for intent classification
    pub model: String,           // default: "claude-haiku-4-5-20251001"
    /// Timeout for L2 call in seconds
    pub timeout_secs: u64,       // default: 5
    /// API URL override (None = default "https://api.anthropic.com")
    pub api_url: Option<String>,
}
```

**Note on coupling**: L2 enablement is decoupled from extractor configuration. A user might configure an extractor for memory storage but not want L2 classification adding latency to recall. `haiku_l2_enabled` defaults to `false` and requires explicit opt-in.

**Wiring into `MemoryConfig`**: Add as a field on `MemoryConfig` alongside existing sub-configs:

```rust
// In config.rs MemoryConfig:
pub struct MemoryConfig {
    pub association: AssociationConfig,
    pub triple: TripleConfig,
    pub promotion: PromotionConfig,
    // ... existing fields ...
    #[serde(default)]
    pub intent_classification: IntentClassificationConfig,
}

impl Default for IntentClassificationConfig {
    fn default() -> Self {
        Self {
            haiku_l2_enabled: false,
            model: "claude-haiku-4-5-20251001".to_string(),
            timeout_secs: 5,
            api_url: None,
        }
    }
}
```

### 2.4.1 Memory Struct Wiring

**Field replacement**: In the `Memory` struct, replace:
```rust
// OLD
intent_anchors: Option<crate::query_classifier::IntentAnchors>
// NEW
intent_classifier: Option<HaikuIntentClassifier>
```

**Construction**: `HaikuIntentClassifier` is constructed via `auto_configure_intent_classifier()`, a new method on `Memory` that follows the same pattern as `auto_configure_extractor()`. It reads the same env vars (`ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_API_KEY`) and is called from the same 4 call sites (lines 148, 206, 262, 323 — immediately after `auto_configure_extractor()`).

`HaikuIntentClassifier` is NOT built inside `Memory::new()` because `Memory::new()` receives only `(path, Option<MemoryConfig>)` — auth tokens are not available there. Auth is configured via env vars read by the `auto_configure_*()` methods called after construction.

```rust
// New method on Memory:
pub fn auto_configure_intent_classifier(&mut self) {
    let config = self.config.intent_classification.clone();
    if !config.haiku_l2_enabled {
        return;
    }
    // Reads same env vars as auto_configure_extractor()
    if let Some(token) = std::env::var("ANTHROPIC_AUTH_TOKEN").ok()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
    {
        let is_oauth = std::env::var("ANTHROPIC_AUTH_TOKEN").is_ok();
        self.intent_classifier = Some(HaikuIntentClassifier::new(
            Box::new(StaticToken(token)),
            is_oauth,
            config.model.clone(),
            config.api_url.clone(),
            config.timeout_secs,
        ));
    }
}
```

Called from the same 4 sites as `auto_configure_extractor()`:
```rust
// Example call site:
memory.auto_configure_extractor();
memory.auto_configure_intent_classifier();  // NEW — same env vars, same pattern
```

Unlike `IntentAnchors` (which was lazy-initialized from the embedding provider on first recall), `HaikuIntentClassifier` needs a `TokenProvider` and HTTP client at construction. These come from the same env var source as the extractor, but as independent instances — there is no way to reach inside `Box<dyn MemoryExtractor>` to extract them.

### 2.5 Remove `IntentAnchors` (cleanup)

Delete `IntentAnchors` struct and all embedding-anchor related code from `query_classifier.rs`:
- `IntentAnchors::new()`
- `IntentAnchors::classify()`
- `INTENT_ANCHOR_THRESHOLD`
- `average_embeddings()`
- All `IntentAnchors` tests (replace with Haiku classifier tests)
- Remove `classify_query_with_embedding()` — replaced by `classify_query_with_l2()`

**Memory struct field replacement**:
- Remove `intent_anchors: Option<IntentAnchors>` field from `Memory` struct
- Add `intent_classifier: Option<HaikuIntentClassifier>` (see §2.4.1)
- Remove the lazy-initialization block in recall path (lines ~1769-1776) that creates `IntentAnchors` from embedding provider
- Update `memory.rs` recall path to pass `self.intent_classifier.as_ref()` to `classify_query_with_l2()`

---

## 3. Data Flow

```
User query arrives at memory.recall()
  │
  ├─ 1. L1: classify_intent_regex(query)
  │       → ~88% of queries get a definite intent here
  │
  ├─ 2. If intent == General AND haiku_l2_enabled:
  │       → L2: haiku_classifier.classify(query)
  │       → 5s timeout, fallback to General on error
  │       → ~12% of queries hit this path
  │
  ├─ 3. Build TypeAffinity from final intent
  │
  ├─ 4. Run 7-channel scoring (existing)
  │
  └─ 5. Apply affinity_multiplier = type_affinity[memory.memory_type]
         final_score = combined_score × affinity_multiplier
```

---

## 4. Error Handling

- **Haiku API timeout**: Return `QueryIntent::General` (neutral affinity, no degradation)
- **Haiku API auth failure**: Log warning, disable L2 for remaining session, use regex only
- **Haiku response parse failure**: Return `QueryIntent::General`
- **Network error**: Return `QueryIntent::General`, log warning

All failure modes fall back to neutral — the system never gets **worse** from a failed L2 call.

---

## 5. Testing Strategy

### Unit Tests (query_classifier.rs)
- Test expanded regex patterns (the 4 fixed cases + new Chinese patterns)
- Verify regex patterns compile and match correctly via `regex::RegexSet`
- Test `HaikuIntentClassifier::parse_response()` with various LLM outputs
- Test timeout/error handling returns General
- Test `disabled` flag is set on auth failure and checked before calls
- Test `classify_query_with_l2()` end-to-end with mock HTTP

### Integration Tests
- Test that `memory.recall()` applies type affinity correctly with L1 only
- Test that L2 fallback fires only when L1 returns General

### Pre-existing test regression
Investigate and fix `somatic_marker_boosts_emotional_memories` test — the type-affinity multiplier for emotional memories under non-Event intents is 0.3 (Definition, HowTo), which may suppress emotional memories that the somatic marker test expects to be boosted. Fix: set intent to `General` or `Event` for that test scenario so emotional memories get neutral affinity.

### The 18-query benchmark from ISS-018 investigation

**Note**: The investigation tested only 12 queries in its regex table, not 18. The design's accuracy claims (78% → 88%) are based on these 12. The complete 18-query benchmark table should be constructed and validated during implementation — all 18 queries must be tested with expected vs. actual results for both L1 regex and L2 Haiku paths.

Re-run all 18 queries and verify:
- Previously failing 4 → now correct via regex fix
- Remaining edge cases → correct via Haiku L2
- No regressions on the queries that already worked
- Document any queries from the claimed 18 that are unaccounted for in the investigation

---

## 6. Implementation Order

0. **Extract shared `anthropic_client` module** — `build_anthropic_headers()` utility, public `StaticToken`, and `pub const DEFAULT_ANTHROPIC_API_URL: &str = "https://api.anthropic.com"`. Refactor `extractor.rs` and `triple_extractor.rs` to use it. This is a **prerequisite** for step 2.
1. **Expand regex patterns** — switch to `regex::RegexSet` matching, fix `contains()` bug, add new patterns. Immediate accuracy boost.
2. **Add `HaikuIntentClassifier` struct** — uses shared `anthropic_client` module, includes `disabled: AtomicBool`
3. **Wire into `classify_query_with_l2()`** — replace embedding L2 path, rename function
4. **Add config** — `IntentClassificationConfig` in config.rs (with `api_url`, default `haiku_l2_enabled: false`)
5. **Wire into `memory.rs`** — replace `intent_anchors` field with `intent_classifier`, add `auto_configure_intent_classifier()` method, call from same 4 sites as `auto_configure_extractor()`, pass to recall
6. **Remove `IntentAnchors`** — dead code cleanup
7. **Tests** — expand existing test suite, fix somatic marker test, build complete 18-query benchmark
8. **Benchmark** — re-run the 18-query suite, measure p95 recall latency (parallel trigger: >500ms)
