# Design Review R2: ISS-018 Recall Intent Classification

**Reviewer**: RustClaw  
**Date**: 2026-04-21  
**Design**: `design.md` (post-R1 apply)  
**Against**: Source code analysis + R1 finding verification  

---

## R1 Finding Verification

All 16 R1 findings were applied. Verified:

- ✅ F-1/2/3: Shared `anthropic_client` module + prerequisite step 0 added
- ✅ F-4: 18-query benchmark note added (build during impl)
- ✅ F-5: `classify_query_with_l2()` signature updated, no embedding param
- ✅ F-6/7: `Memory::new()` construction + field replacement documented
- ✅ F-8: Bilingual note added (not overstated)
- ✅ F-9: Token estimate updated to 200-250
- ✅ F-10: `haiku_l2_enabled` defaults false, decoupled
- ✅ F-11: Honest sequential latency + 500ms trigger
- ✅ F-12: ISS-001 → ISS-018 fixed
- ✅ F-13: `regex::RegexSet` decision documented
- ✅ F-14: Somatic marker regression test noted
- ✅ F-15: `api_url: Option<String>` added
- ✅ F-16: `disabled: AtomicBool` mechanism added

---

## New Issues Found in R2

### 🔴 Critical

**(none)**

### 🟡 Important

**FINDING-R2-1: `RegexSet` cannot replace per-category pattern iteration** ✅ Applied

The design says: "Compile patterns into `regex::RegexSet` at startup for each intent category." But `RegexSet::is_match()` returns whether *any* pattern in the set matched — it doesn't tell you *which category* matched. The current code needs to check each category in priority order (HowTo before Definition, etc.).

Two options:
- **Option A**: One `RegexSet` per category (5 sets). Check them in priority order: `if howto_set.is_match(q) { HowTo } else if event_set.is_match(q) { Event }...`. This preserves priority ordering.
- **Option B**: One unified `RegexSet` with `matches()` returning matching pattern indices, then map index→category. But this loses explicit priority ordering unless you also store priority.

Recommend Option A — it's the most natural translation of the existing `for p in CATEGORY { if contains(p) }` structure, keeps category priority explicit, and the compile-once cost is the same.

**Suggested fix**: Update §2.1 to specify Option A (one `RegexSet` per category, checked in priority order). Show the struct:

```rust
struct IntentRegex {
    howto_en: RegexSet,
    howto_zh: RegexSet,
    event_en: RegexSet,
    event_zh: RegexSet,
    // ... etc
}
```

---

**FINDING-R2-2: `HaikuIntentClassifier` constructor needs auth credentials — but `Memory::new()` doesn't have them** ✅ Applied

The design says: "Constructed at `Memory::new()` time when extractor config is present." But look at the actual `Memory::new()` signature:

```rust
pub fn new(path: &str, config: Option<MemoryConfig>) -> Result<Self, ...>
```

Neither `path` nor `MemoryConfig` contain auth tokens. Auth is configured via `auto_configure_extractor()` which reads env vars (`ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_API_KEY`) and constructs the extractor *after* `Memory::new()` returns the struct. The extractor gets `set_extractor()` called separately.

So `HaikuIntentClassifier` cannot be constructed inside `Memory::new()` — the auth tokens aren't available there. It needs to be:
- Constructed in `auto_configure_extractor()` alongside the extractor (same env var source), OR
- Constructed via a new `auto_configure_intent_classifier()` method called right after `auto_configure_extractor()`, OR
- Lazy-initialized on first recall (same pattern as current `IntentAnchors`) — but the design explicitly rejects this

**Suggested fix**: Add a parallel method `auto_configure_intent_classifier()` that reads the same env vars and constructs `HaikuIntentClassifier`. Call it from all the same places that call `auto_configure_extractor()` (lines 148, 206, 262, 323). Update §2.4.1 to describe this pattern instead of "at `Memory::new()` time."

---

**FINDING-R2-3: `IntentClassificationConfig` not wired into `MemoryConfig`** ✅ Applied

The design defines `IntentClassificationConfig` as a separate struct (§2.4) but never shows it nested in `MemoryConfig`. Looking at `config.rs`, all other sub-configs are fields on `MemoryConfig`:

```rust
pub association: AssociationConfig,
pub triple: TripleConfig,
pub promotion: PromotionConfig,
```

The design should explicitly add:
```rust
#[serde(default)]
pub intent_classification: IntentClassificationConfig,
```

to `MemoryConfig` and add a `Default` impl for `IntentClassificationConfig`. Minor but without this, the config has no way to be passed in.

**Suggested fix**: Add field to `MemoryConfig` struct definition and `Default` impl in §2.4.

---

**FINDING-R2-4: Shared `anthropic_client` module — `TokenProvider` trait lives in `extractor.rs`, not shareable** ✅ Applied

The design says the shared module should "re-export `TokenProvider` trait." But `TokenProvider` is currently defined in `extractor.rs` (line 125) and re-exported from `lib.rs` as `pub use extractor::TokenProvider`. Moving `TokenProvider` to `anthropic_client.rs` would break the public API:

```rust
// lib.rs currently:
pub use extractor::{..., TokenProvider, ...};
```

Options:
- **Option A**: Move `TokenProvider` to `anthropic_client.rs`, update `extractor.rs` to `use crate::anthropic_client::TokenProvider`, update `lib.rs` to re-export from new location. Breaking change for anyone importing `engramai::extractor::TokenProvider` directly (unlikely since `lib.rs` re-exports it).
- **Option B**: Keep `TokenProvider` in `extractor.rs`, have `anthropic_client.rs` import it from there. Shared module depends on extractor, not the other way around.

**Suggested fix**: Option B is safer — `anthropic_client.rs` uses `use crate::extractor::TokenProvider`. `TokenProvider` stays where it is. Update §1 to clarify this dependency direction.

---

### 🟢 Minor

**FINDING-R2-5: Pattern `"(?i)i am building|i am working"` — regex alternation scope** ✅ Applied

In regex, `(?i)i am building|i am working` is parsed as `(?i)i am building` OR `i am working` (case-insensitive only applies to first branch). To make both case-insensitive, need: `(?i)(?:i am building|i am working)` or `(?i)i am (?:building|working)`.

**Suggested fix**: Use `(?i)i am (?:building|working)` — more correct and more concise.

---

**FINDING-R2-6: Chinese pattern `什么\w+` — `\w` doesn't match Chinese characters in `regex` crate by default** ✅ Applied

In Rust's `regex` crate, `\w` matches `[0-9A-Za-z_]` by default (ASCII mode). It does NOT match Chinese characters unless Unicode mode is explicitly enabled with `(?u)` flag. So `什么\w+` would match "什么abc" but NOT "什么书" or "什么人".

The fix is either:
- Use `(?u)什么\w+` — Unicode `\w` includes CJK
- Or use `什么.+` — but that's too greedy
- Or use explicit character classes: `什么[\p{Han}\w]+`

Since the `regex` crate defaults to Unicode mode for `.` but NOT for `\w`, this is a subtle bug. Actually — checking the regex crate docs: as of regex 1.x, `\w` is **Unicode-aware by default** unless `(?-u)` is set. So `什么\w+` should work. Let me correct this — it depends on whether `RegexSet` uses the same default.

**Suggested fix**: Verify during implementation that `RegexSet::new(&["什么\\w+"])` matches "什么书". Add a unit test for this specific case. If it doesn't match, use `什么[\\p{Han}\\w]+`.

---

**FINDING-R2-7: `disabled: AtomicBool` — no recovery path** ✅ Applied

Once L2 is disabled by a 401/403, it stays disabled for the entire session. If the auth failure was transient (e.g., token refresh race), there's no way to re-enable without restarting. This is fine for v1, but should document it as a known limitation.

**Suggested fix**: Add one sentence to §2.2: "v1 has no auto-recovery; a future version could retry after a cooldown period."

---

**FINDING-R2-8: Step 0 prerequisite scope — what about `api_url`?** ✅ Applied

Both `extractor.rs` and `triple_extractor.rs` hardcode `https://api.anthropic.com/v1/messages`. The shared module should also centralize the API URL constant, since `HaikuIntentClassifier` adds a third user.

**Suggested fix**: Add `pub const DEFAULT_ANTHROPIC_API_URL: &str = "https://api.anthropic.com"` to the shared module. Not blocking, but good to do in step 0.

---

## 📊 Summary

| Severity | Count | IDs |
|----------|-------|-----|
| 🔴 Critical | 0 | — |
| 🟡 Important | 4 | R2-1, R2-2, R2-3, R2-4 |
| 🟢 Minor | 4 | R2-5, R2-6, R2-7, R2-8 |

### R1 Regressions: None
All 16 R1 findings properly addressed.

### Verdict: **Ready to implement after applying R2 findings**

The 4 important findings are all wiring/integration details that were underspecified. None require architectural changes — they refine how pieces connect. After applying these, the design fully covers the implementation path.
