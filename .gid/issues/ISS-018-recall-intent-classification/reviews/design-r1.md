# Design Review: ISS-018 Recall Intent Classification

**Reviewer**: coder  
**Date**: 2026-04-21  
**Design**: `design.md` (rev 1)  
**Against**: `investigation.md`, source code analysis  

---

## 🔴 Critical

### FINDING-1: Design cannot reuse extractor's TokenProvider — no shared access path

The design says HaikuIntentClassifier will "reuse existing `AnthropicExtractor` HTTP infrastructure" and "piggyback on the existing extractor's `TokenProvider` — no new auth config needed." This is architecturally impossible as currently structured.

**Evidence**: In `memory.rs`, the `Memory` struct stores the extractor as `Option<Box<dyn MemoryExtractor>>` (line ~94 area). The `MemoryExtractor` trait (in `extractor.rs`) exposes only `fn extract(&self, text: &str) -> Result<Vec<ExtractedFact>, ...>`. There is no way to reach inside the trait object to access the `TokenProvider`, `is_oauth` flag, `client`, or `api_url` from the `AnthropicExtractor` concrete type.

The `triple_extractor.rs` faced the same problem and solved it by having callers construct a **completely separate** `AnthropicTripleExtractor` with its own `TokenProvider` and `reqwest::blocking::Client`. It does NOT share the extractor's client or token provider — it duplicates them.

**Impact**: The design's claim of "no new auth config" is wrong. The `HaikuIntentClassifier` will need its own `TokenProvider` instance (either passed during `Memory` construction or cloned). The design must specify **how** the token provider reaches the classifier — the current design hand-waves this critical wiring detail.

**Fix**: Follow the `triple_extractor.rs` pattern explicitly. Either:
- (a) Accept a separate `Box<dyn TokenProvider>` + `is_oauth` at `Memory::new()` time for intent classification, or
- (b) Add a `TokenProvider` cloning mechanism / `Arc<dyn TokenProvider>`, or
- (c) Accept that `HaikuIntentClassifier` is constructed by the caller and passed into `Memory` (like extractor is).

### FINDING-2: `build_headers()` is duplicated across 3 call sites — design adds a 4th

`extractor.rs::AnthropicExtractor::build_headers()`, `triple_extractor.rs::AnthropicTripleExtractor::build_headers()`, and now the proposed `HaikuIntentClassifier` would each independently implement the same OAuth/API-key header logic (anthropic-beta, Authorization Bearer, x-api-key, user-agent spoofing, etc.).

**Evidence**: The `build_headers()` in `triple_extractor.rs` (lines ~155-185) is a near-verbatim copy of `extractor.rs` (lines ~220-255). The design proposes yet another copy inside `HaikuIntentClassifier`.

**Impact**: Three copies of security-sensitive header construction code. If OAuth headers change (e.g., anthropic-beta version bump), all three must be updated in lockstep. This is a bug factory.

**Fix**: Extract a shared `AnthropicClient` or `fn build_anthropic_headers(token: &str, is_oauth: bool) -> HeaderMap` utility. The design should mandate this refactor as a prerequisite or at minimum as a follow-up task.

### FINDING-3: `StaticToken` is duplicated — design adds a 3rd copy

`extractor.rs` defines `struct StaticToken(String)` implementing `TokenProvider`. `triple_extractor.rs` defines its own identical `struct StaticToken(String)`. The design would create a third.

**Impact**: Same as FINDING-2 — code duplication of auth primitives.

**Fix**: Make `StaticToken` public in `extractor.rs` or create a shared auth module.

---

## 🟡 Important

### FINDING-4: Design only claims 4 regex failures but investigation table shows 4/18 — the remaining 14 are NOT all verified

The investigation tested only 12 queries in the regex table (not 18). The design claims "14 that already worked" but the investigation table only shows 12 entries with 8 passes. The "18-query benchmark" is referenced in the testing strategy (Section 5) but there is no complete 18-query table anywhere. Six queries are unaccounted for.

**Impact**: The design's accuracy claims (78% → 88%) are based on incomplete data. The actual regex coverage gap may be larger than estimated.

**Fix**: The design should include the complete 18-query benchmark table with expected vs. actual results for all 18 queries, and the regex improvements should be validated against all 18 — not just the 4 known failures.

### FINDING-5: `classify_query_with_embedding` signature change not fully specified

The design says to "Update `classify_query_with_embedding()` signature to use `HaikuIntentClassifier`" but doesn't specify the new signature. The current signature is:

```rust
pub fn classify_query_with_embedding(
    query: &str,
    query_embedding: Option<&[f32]>,
    anchors: Option<&IntentAnchors>,
) -> QueryAnalysis
```

The new classifier doesn't need `query_embedding` at all (Haiku uses text, not embeddings). But `memory.rs` currently passes `Some(&query_embedding)` at line 1778. If the signature drops the embedding parameter, the caller changes too.

**Impact**: The design doesn't specify whether the new function takes `Option<&HaikuIntentClassifier>` or whether a new function name is used. This ambiguity could lead to a confusing API where `classify_query_with_embedding` no longer uses embeddings.

**Fix**: Design should specify a renamed function (e.g., `classify_query_with_l2(query, haiku_classifier: Option<&HaikuIntentClassifier>)`) and the corresponding caller changes in `memory.rs`.

### FINDING-6: Lazy initialization pattern in `memory.rs` not addressed for new classifier

Currently `IntentAnchors` is lazy-initialized in `memory.rs` recall path (lines 1769-1776): "if `self.intent_anchors.is_none()` → create from embedding provider." The design says to remove `IntentAnchors` and use `HaikuIntentClassifier`, but doesn't specify the initialization strategy.

**Evidence**: `HaikuIntentClassifier` needs a `TokenProvider` and HTTP client. Unlike `IntentAnchors` (which could be created from the already-available embedding provider), the classifier needs auth credentials that may not be available at recall time.

**Impact**: If the classifier is lazy-initialized on first recall, it needs access to auth config. If it's initialized at `Memory::new()` time, the `Memory` constructor needs new parameters. Neither path is specified.

**Fix**: Design should explicitly state: classifier is constructed during `Memory::new()` (or `set_extractor()` time) and stored as `Option<HaikuIntentClassifier>` on the `Memory` struct. Show the wiring.

### FINDING-7: The `intent_anchors` field on `Memory` struct needs to be replaced, not just deleted

`memory.rs` line 94: `intent_anchors: Option<crate::query_classifier::IntentAnchors>` is a field on the `Memory` struct. The design's Section 2.5 says to delete `IntentAnchors` but doesn't mention replacing this field with `Option<HaikuIntentClassifier>` or adding it elsewhere.

**Impact**: Without specifying the new field, the wiring between `Memory` and the classifier is undefined.

**Fix**: Section 2.5 should list: "Replace `intent_anchors: Option<IntentAnchors>` field in `Memory` struct with `haiku_classifier: Option<HaikuIntentClassifier>`."

### FINDING-8: Haiku prompt doesn't handle bilingual queries

The prompt in Section 2.2 says `Query: "{query}"` but gives all instructions in English. The investigation specifically identified Chinese query classification as a key requirement (中文 pattern 覆盖率不够). The investigation's selling point for Haiku was "中英文都好" but that assumes the prompt works bilingually.

**Impact**: While Haiku likely handles Chinese fine with an English prompt, the design doesn't validate this. The investigation never actually tested Haiku (marked as "未实测").

**Fix**: Either (a) add a Chinese instruction line to the prompt, (b) add a note that the English-only prompt has been validated with Chinese queries, or (c) acknowledge this as a risk requiring testing in Phase 3.

### FINDING-9: Cost estimate in design is wrong by ~2.5x

Design Section 2.2 says: "~160 input + 5 output = 165 tokens × $0.25/$1.25 per MTok = ~$0.00004/call"

Math check: 160 × $0.25/1M + 5 × $1.25/1M = $0.00004 + $0.00000625 ≈ $0.000046. That's close enough. But the prompt template alone is ~80 tokens (system text) + query. A typical query of 10-20 tokens puts input at ~100 tokens, not 160. The "~150 input tokens" claim in parentheses contradicts the "~160" in the cost line. Minor but sloppy.

More importantly: with the full message structure (`{"model":..., "messages":[...]}`) and Anthropic's token counting (which includes message framing), actual input tokens are likely ~200-250, making the cost ~$0.00006/call. Still negligible, but the estimate should be accurate.

**Fix**: Run one actual Haiku call with the prompt and report real token counts.

### FINDING-10: No design for disabling L2 when no extractor is configured

The design says `haiku_l2_enabled` defaults to `true if extractor is configured`. But the extractor is for *memory extraction* (store path), not recall. A user might configure an extractor for storage but not want L2 intent classification adding latency to every General-classified recall.

**Impact**: Coupling L2 intent classification enablement to extractor configuration is an implicit dependency that may surprise users.

**Fix**: Make `haiku_l2_enabled` default to `false` and require explicit opt-in. Or at minimum, document why it's coupled to extractor presence.

### FINDING-11: Sequential decision contradicts investigation's parallelism argument

The design decides "Start with sequential (simpler)" for the Haiku call. But the investigation's key argument for choosing Haiku over alternatives was: "延迟被 recall embedding 计算掩盖（并行）" — latency is hidden by parallelizing with embedding computation.

If implemented sequentially, the recall path becomes: regex (~0ms) → Haiku (~200ms) → embed (~200ms) → scoring. That's 200ms *added* to every General-classified query (~12% of queries). The investigation justified Haiku's 200ms latency specifically because it could be parallelized away.

**Impact**: The sequential implementation negates the key latency argument for choosing Haiku. This should at minimum be called out as a known regression from the ideal, with a concrete trigger for when to implement parallelism.

**Fix**: Either commit to parallel execution in v1, or restate the latency impact honestly: "~200ms added to ~12% of queries" and set a concrete latency threshold that triggers parallel implementation.

---

## 🟢 Minor

### FINDING-12: Design references issue as "ISS-001" inconsistently

The design file is at `.gid/issues/ISS-018-recall-intent-classification/` but the document header says "Traces to: ISS-001-recall-intent-classification.md" and the investigation title uses "ISS-001". The issue number is inconsistent between directory name (ISS-018) and document content (ISS-001).

**Fix**: Standardize to ISS-018 throughout.

### FINDING-13: `INTENT_RELATIONAL_ZH` contains a regex-style pattern that isn't used as regex

In `query_classifier.rs` line ~538: `"对.*的看法"` is listed in `INTENT_RELATIONAL_ZH` but the matching code uses `query.contains(p)`, which does **literal** string matching, not regex. The pattern `"对.*的看法"` will never match because no query literally contains the characters `.*`.

**Impact**: This is a pre-existing bug, not introduced by the design. But the design's "expand regex patterns" section doesn't mention fixing it, and the newly proposed patterns (e.g., `怎么\w+`) would have the same problem if added as string literals to the existing `contains()` arrays.

**Fix**: The design should clarify: are the new patterns (like `怎么\w+`) implemented as actual `regex::Regex` patterns or as string literals? If string literals, `怎么\w+` won't work. If regex, the existing matching code needs to change from `contains()` to regex matching. This is a **design gap** for the regex enhancement section.

### FINDING-14: Test strategy doesn't cover the somatic marker test regression

The investigation notes: "⚠️ 1 个 pre-existing 测试失败 (`somatic_marker_boosts_emotional_memories`) — 可能受 type affinity 影响." The design's testing strategy (Section 5) doesn't mention investigating or fixing this regression.

**Impact**: The type-affinity multiplier for emotional memories under non-Event intents is 0.3 (Definition, HowTo). This could cause the somatic marker test to fail if it implicitly relies on emotional memories not being suppressed.

**Fix**: Add to test strategy: "Investigate and fix `somatic_marker_boosts_emotional_memories` test — likely needs intent set to General or Event for the test scenario."

### FINDING-15: `IntentClassificationConfig` has no `api_url` field

The design's config struct (Section 2.4) has `haiku_l2_enabled`, `model`, and `timeout_secs`. But `HaikuIntentClassifier` (Section 2.2) has an `api_url` field. The config doesn't expose this, meaning the API URL can't be overridden for testing or proxy setups.

**Fix**: Add `api_url: Option<String>` to `IntentClassificationConfig` (None = inherit from extractor or use default).

### FINDING-16: Error handling for "auth failure disables L2 for remaining session" is unspecified

Section 4 says: "Haiku API auth failure: Log warning, disable L2 for remaining session, use regex only." This requires mutable state on the classifier (a disabled flag) or on the `Memory` struct. Neither the `HaikuIntentClassifier` struct nor the `Memory` integration specifies how this flag is stored or checked.

**Fix**: Add an `AtomicBool` or `Cell<bool>` field to `HaikuIntentClassifier` for the disabled state, or clarify the mechanism.

---

## Summary

| Severity | Count | IDs |
|----------|-------|-----|
| 🔴 Critical | 3 | FINDING-1, FINDING-2, FINDING-3 |
| 🟡 Important | 8 | FINDING-4 through FINDING-11 |
| 🟢 Minor | 5 | FINDING-12 through FINDING-16 |
| **Total** | **16** | |

### Key Themes

1. **Auth/HTTP infrastructure sharing is the biggest gap** (FINDING-1, 2, 3). The design assumes reuse but the codebase doesn't support it. The `triple_extractor.rs` precedent shows the actual pattern is full duplication, which the design should either accept (and call out) or fix with a shared utility.

2. **Wiring between Memory and classifier is undefined** (FINDING-5, 6, 7). The design specifies the classifier struct but not how it gets constructed, stored, or accessed from the recall path.

3. **Regex pattern syntax mismatch** (FINDING-13). The proposed "regex" patterns like `怎么\w+` cannot work with the existing `contains()` matching — this must be resolved before implementation.

4. **Latency argument is self-contradictory** (FINDING-11). Investigation justifies Haiku via parallelism; design chooses sequential. Pick one story.

---

## Resolution

**All 16 findings applied to design.md on 2026-04-21.**

### FINDING-1 ✅ Applied
- Section: §1 Architecture Change
- Change: Added "Note on auth wiring" explaining `MemoryExtractor` trait limitation. Design now states `HaikuIntentClassifier` constructs its own `TokenProvider` and client, following `triple_extractor.rs` pattern.

### FINDING-2 ✅ Applied
- Section: §1 Architecture Change, §6 Implementation Order
- Change: Added prerequisite refactor: extract shared `anthropic_client` module with `build_anthropic_headers()` utility. Step 0 in implementation order.

### FINDING-3 ✅ Applied
- Section: §1 Architecture Change
- Change: Shared module includes `pub struct StaticToken` — single canonical definition. All three consumers import from shared module.

### FINDING-4 ✅ Applied
- Section: §5 Testing Strategy
- Change: Added note that investigation only tested 12 queries (not 18). Full 18-query benchmark must be constructed and validated during implementation.

### FINDING-5 ✅ Applied
- Section: §2.2 Haiku Intent Classifier
- Change: Added explicit new function signature `classify_query_with_l2(query, haiku_classifier: Option<&HaikuIntentClassifier>)`. Embedding parameter dropped.

### FINDING-6 ✅ Applied
- Section: §2.4.1 Memory Struct Wiring
- Change: Added explicit construction strategy — classifier built at `Memory::new()` time (not lazy-init), with code example showing auth wiring.

### FINDING-7 ✅ Applied
- Section: §2.4.1, §2.5
- Change: Explicitly stated field replacement: `intent_anchors: Option<IntentAnchors>` → `intent_classifier: Option<HaikuIntentClassifier>`. Listed in both config and cleanup sections.

### FINDING-8 ✅ Applied
- Section: §2.2 Prompt
- Change: Added bilingual note to prompt: "The query may be in English or Chinese — classify based on meaning, not language." Added bilingual example queries. Did not overstate Chinese as primary — both languages covered equally.

### FINDING-9 ✅ Applied
- Section: §2.2 Cost estimate
- Change: Updated estimate to ~200-250 input tokens (including message framing). Conservative $0.00007/call. Added note to validate with real Haiku call during implementation.

### FINDING-10 ✅ Applied
- Section: §2.4 Configuration
- Change: `haiku_l2_enabled` now defaults to `false` (explicit opt-in). Added note explaining decoupling from extractor configuration.

### FINDING-11 ✅ Applied
- Section: §1 Solution, §2.3 (renamed from "Async Parallel Execution" to "Sequential Execution with Parallel Upgrade Path")
- Change: Honestly states sequential adds ~200ms to ~12% of queries. Sets concrete trigger: p95 > 500ms → implement parallel. Removed misleading "parallelized" claim from Solution summary.

### FINDING-12 ✅ Applied
- Section: §Title, header
- Change: Standardized to ISS-018 throughout (was ISS-001 in title and traces-to line).

### FINDING-13 ✅ Applied
- Section: §2.1 Regex Pattern Enhancement
- Change: Decision to switch from `contains()` to `regex::RegexSet`. Documents pre-existing bug (`"对.*的看法"` never matched). All new patterns written as actual regex. Implementation note: compile once at startup.

### FINDING-14 ✅ Applied
- Section: §5 Testing Strategy
- Change: Added "Pre-existing test regression" subsection for `somatic_marker_boosts_emotional_memories` test with fix approach (set intent to General/Event for that test).

### FINDING-15 ✅ Applied
- Section: §2.4 Configuration
- Change: Added `api_url: Option<String>` field to `IntentClassificationConfig` (None = default).

### FINDING-16 ✅ Applied
- Section: §2.2 Haiku Intent Classifier
- Change: Added `disabled: AtomicBool` field to struct. Added "Disabled state" paragraph explaining the mechanism (set on 401/403, checked via `Relaxed` ordering before each call).
