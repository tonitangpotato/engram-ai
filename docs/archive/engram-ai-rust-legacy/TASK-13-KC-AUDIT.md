# TASK-13: Knowledge Compiler Audit — Findings

**Date**: 2026-04-20
**Scope**: `src/compiler/` (21 files, 15,261 lines), CLI integration (`src/main.rs`), integration test (`tests/kc_integration_test.rs`)

---

## Summary

The Knowledge Compiler is a substantial, well-structured module with solid test coverage (302 unit tests pass). The architecture — trait-based storage, pluggable LLM providers, graceful degradation — is sound. However, the audit uncovered **1 critical bug**, **3 important issues**, and **4 minor issues**.

---

## 🔴 Critical

### C-1: Integration test is broken — does not compile

**File**: `tests/kc_integration_test.rs:61`
**Issue**: The `make_memory()` helper creates a `MemorySnapshot` without the `embedding` field, which was added to the struct after the test was written. The test is gated behind `#[cfg(feature = "kc")]` and the `kc` feature is never enabled in the default test command (`cargo test --workspace`), so this breakage has been silently hiding.

```
error[E0063]: missing field `embedding` in initializer of `MemorySnapshot`
  --> tests/kc_integration_test.rs:61:5
```

**Fix**: Add `embedding: None,` to the `make_memory()` helper in `kc_integration_test.rs`.

**Severity**: Critical — the only e2e integration test for KC doesn't compile, meaning the full pipeline (memories → compile → export) has zero integration test coverage right now.

---

## 🟡 Important

### I-1: Feature flag `kc` is declared but never enforced

**Files**: `Cargo.toml:60`, `src/compiler/mod.rs:4`, `src/lib.rs:95`
**Issue**: `mod.rs` says "Gated behind the `kc` feature flag" but the module is unconditionally compiled (`pub mod compiler;` in `lib.rs` has no `#[cfg(feature = "kc")]`). The `kc` feature in `Cargo.toml` is empty (`kc = []`). The integration test IS gated behind `#[cfg(feature = "kc")]`, which is why it's been silently broken.

**Options**:
1. **Remove the feature gate claim** — update `mod.rs` comment and `Cargo.toml` to remove the misleading `kc` feature. The compiler is always available.
2. **Actually gate the module** — add `#[cfg(feature = "kc")]` to `src/lib.rs:95`. This would be a breaking change.

**Recommendation**: Option 1 (remove the dead feature flag). The module is already always compiled and tested; pretending it's optional adds confusion. Then remove `#[cfg(feature = "kc")]` from `kc_integration_test.rs` so it runs in normal `cargo test --workspace`.

### I-2: CLI `query` command duplicates API logic

**File**: `src/main.rs:1869-1890`
**Issue**: The CLI `knowledge query` command reimplements topic search (list all → filter by substring → take N) instead of using `MaintenanceApi::query()`, which has better relevance scoring (title match weighted 3x, summary 2x, content 1x, density bonus). The CLI version is simpler but produces inferior results — no relevance ranking, just first-N-matched.

Other CLI commands (`compile`, `health`, `decay`) correctly use their respective engines. `query` is the outlier.

**Fix**: Replace the inline search with `MaintenanceApi::query(&search, &QueryOpts { limit, .. })`.

### I-3: `--repair` flag in `audit` command is a no-op

**File**: `src/main.rs` (audit command handler)
**Issue**: The `--repair` flag prints `"⚠️ Auto-repair is not yet implemented"` but the `HealthAuditor` already has `suggest_repair()` and `repair_link()` methods. The infrastructure exists in `src/compiler/health.rs` but isn't wired into the CLI.

**Fix**: Wire `HealthAuditor::suggest_repair()` + `repair_link()` into the `--repair` path.

---

## 🔵 Minor

### M-1: HTML export returns an error

**File**: `src/compiler/export.rs:85`
**Issue**: `ExportFormat` has only `Json` and `Markdown`. If HTML is ever passed, it returns `KcError::ExportError("HTML export is not yet supported")`. This is fine as long as the CLI doesn't expose HTML as an option — and it doesn't currently. Low priority.

### M-2: `mod.rs` comment lists "future submodules" that already exist

**File**: `src/compiler/mod.rs:10-20`
**Issue**: The comment says "Future submodules (not yet implemented)" and then lists `discovery`, `compilation`, `trigger`, `lifecycle`, `feedback`, `decay`, `conflict`, `health`, `export`, `access`, `privacy`, `llm`, `intake` — all of which are implemented and have tests. The comment is stale.

**Fix**: Remove the "future" comment or update to reflect reality.

### M-3: `topic_lifecycle::execute_split()` returns stub pages

**File**: `src/compiler/topic_lifecycle.rs:144`
**Issue**: Comment says "Returns new TopicPages (stubs — need LLM compilation)". The split operation creates pages with content derived from memory subsets but doesn't trigger recompilation. This is by design (split creates candidates, then the user runs `compile`), but could be documented more clearly in the CLI help.

### M-4: Watcher module has no CLI exposure

**File**: `src/compiler/watcher.rs` (891 lines, well-tested)
**Issue**: `DirectoryWatcher` polls an inbox directory for new files (markdown, txt, json, audio) and auto-imports them. This is a powerful feature but has no CLI command — there's no `engram knowledge watch` or daemon mode. Currently only usable programmatically.

**Impact**: Low — this is likely designed for library consumers (like RustClaw), not direct CLI use. But worth noting since it's a significant chunk of code (891 lines) with no user-facing entry point.

---

## Test Coverage Summary

| Module | Unit Tests | Status |
|--------|-----------|--------|
| api | 17 | ✅ All pass |
| compilation | 14 | ✅ All pass |
| config | 10 | ✅ All pass |
| conflict | 12 | ✅ All pass |
| decay | 11 | ✅ All pass |
| degradation | 8 | ✅ All pass |
| discovery | 9 | ✅ All pass |
| export | 7 | ✅ All pass |
| feedback | 6 | ✅ All pass |
| health | 10 | ✅ All pass |
| import | 10 | ✅ All pass |
| intake | 22 | ✅ All pass |
| llm | 7 | ✅ All pass |
| lock | 5 | ✅ All pass |
| manual_edit | 8 | ✅ All pass |
| privacy | 8 | ✅ All pass |
| storage | 12 | ✅ All pass |
| topic_lifecycle | 10 | ✅ All pass |
| types | 45 | ✅ All pass |
| watcher | 16 | ✅ All pass |
| **kc_integration_test** | **10** | **❌ Does not compile** |
| **Total** | **302 unit + 10 integration** | **302 pass, 10 broken** |

---

## Architecture Positives (things done well)

1. **Clean trait abstraction**: `KnowledgeStore` trait decouples storage from logic — all engines accept `&S: KnowledgeStore` enabling in-memory testing
2. **Graceful degradation**: `degradation.rs` properly detects capability levels (Minimal/Embeddings/Full) and provides actionable messages
3. **Comprehensive types**: `types.rs` (1,600 lines) defines all domain types with serde support and 45 serde roundtrip tests
4. **Privacy system**: Per-topic privacy levels, access control, redaction, audit logging — complete chain
5. **Conflict detection**: Section-level cross-topic conflict analysis with resolution suggestions
6. **Intake pipeline**: Platform-aware content extraction (YouTube, GitHub, etc.) with dedup via content hashing

---

## Recommended Fix Priority

1. **C-1**: Fix integration test (5 min) — add `embedding: None` + remove `#[cfg(feature = "kc")]` gate
2. **I-1**: Clean up dead feature flag (5 min) — remove `kc = []` from Cargo.toml, update mod.rs comment
3. **I-2**: Wire CLI query to API (10 min) — replace inline search with `MaintenanceApi::query()`
4. **I-3**: Wire audit --repair (30 min) — connect existing `repair_link()` to CLI
5. **M-2**: Update stale comment (2 min)
