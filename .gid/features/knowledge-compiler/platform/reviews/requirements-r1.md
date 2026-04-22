# Requirements Review: Platform Setup, LLM Config, Import & Intake

- **Document**: `.gid/features/knowledge-compiler/platform/requirements.md`
- **Master GUARDs**: `.gid/features/knowledge-compiler/requirements.md` (GUARD-1 through GUARD-6)
- **Review depth**: Standard (Phases 0–5, 26 checks)
- **Reviewer**: RustClaw (automated)
- **Date**: 2026-04-17

---

## Summary

| Severity | Count |
|----------|-------|
| Critical | 3 |
| Important | 6 |
| Minor | 4 |
| **Total findings** | **13** |

**Overall assessment**: Strong requirements document — 14 GOALs with clear pass/fail criteria and good separation of concerns. The main issues are: (1) several GOALs leak implementation details (specific tools, config field names, shell commands), (2) a few pass/fail criteria are not fully measurable, and (3) some cross-references to master GUARDs are implicit rather than explicit.

---

## Phase 0: Document Size Check

### Check 0: Document size
**✅ Pass** — 14 GOALs across 4 sections (§5, §5.5, §6, §9). Well within the ≤15 GOAL limit for a single feature document.

---

## Phase 1: Structural Completeness

### Check 1: GOAL/GUARD naming convention
**✅ Pass** — All requirements use `GOAL-plat.N` naming consistently. No stray abbreviations (CR, REQ, FR, etc.). Master GUARDs correctly referenced as GUARD-1 through GUARD-6.

### Check 2: Every GOAL has priority
**✅ Pass** — All 14 GOALs have explicit priority tags: P0 (8), P1 (4), P2 (2). Distribution is reasonable for a platform/infrastructure feature.

### Check 3: Every GOAL has pass/fail criteria
**✅ Pass** — All 14 GOALs have explicit **Pass/fail** blocks with concrete scenarios.

### Check 4: Section numbering consistency
**FINDING-1** ⚠️ Minor
- §5, §5.5, §6, §9 — the numbering has gaps (no §7, §8) and uses a half-step (§5.5). This is because sections are shared with the master doc's numbering scheme, which is fine for cross-referencing. But §5.5 is unusual — confirm this is intentional and matches the master numbering.
- **Suggestion**: No change needed if master doc uses §5.5. Just verify alignment.

### Check 5: Cross-references resolve
**FINDING-2** ⚠️ Important
- The overview says "See [master requirements.md](../requirements.md) for GUARDs (system-wide constraints GUARD-1 through GUARD-6)." This is correct.
- However, **no individual GOAL explicitly maps to which GUARDs it must satisfy**. For example, GOAL-plat.2 defines a config file with `llm_api_key` — this should explicitly reference GUARD-4 ("Secrets never leak into logs or exports") to ensure API keys are handled safely. GOAL-plat.4 auto-installs software — should reference GUARD-6 ("No silent data loss") since auto-install could fail mid-way.
- **Suggestion**: Add a "GUARDs: GUARD-N, GUARD-M" line to GOALs where specific guards are relevant, at minimum: plat.2 → GUARD-4, plat.4 → GUARD-6, plat.12/13 → GUARD-6.

---

## Phase 2: WHAT vs HOW Boundary

### Check 6: Implementation leakage — specific tools/libraries
**FINDING-3** 🔴 Critical
- **GOAL-plat.4** specifies exact installation commands: `brew install ollama`, `curl -fsSL https://ollama.com/install.sh | sh`. These are implementation details — the requirement should state "system auto-installs the embedding runtime on supported platforms" without prescribing the exact shell commands.
- **GOAL-plat.6** specifies `cargo install engram`, `brew install engram`, and "GitHub Releases, macOS arm64/x86 + Linux x86". Distribution channels are product decisions, not requirements. The requirement should be "KC can be installed as a standalone binary on macOS and Linux without requiring RustClaw."
- **Suggestion**: Rewrite both GOALs to state the desired outcome (auto-install works, standalone install works) and move specific commands/channels to a design doc or deployment plan.

### Check 7: Implementation leakage — specific data formats/schemas
**FINDING-4** 🔴 Critical
- **GOAL-plat.2** defines the exact TOML config schema with specific field names (`llm_provider`, `llm_api_key`, `llm_model`, `llm_base_url`, `embedding_provider`, `embedding_model`, `compile_interval`, `db_path`). This is design-level detail. The requirement should state "KC uses a single configuration file for LLM provider selection, API credentials, embedding settings, and compile schedule" without specifying the exact field names or file format.
- **GOAL-plat.7** specifies exact Cargo feature flag names (`core`, `agent`, `emotional`) and their contents. This is build-system implementation detail. The requirement should state "engram crate supports modular feature activation so KC standalone uses only knowledge features while agent frameworks can opt into additional capabilities."
- **Suggestion**: Extract config schema and feature flag design into the design document. Keep requirements at the "what capability is needed" level.

### Check 8: Implementation leakage — specific algorithms/strategies
**✅ Pass** — No algorithm prescriptions found. GOAL-plat.5's fallback chain describes behavior priority (which is WHAT), not a specific algorithm.

### Check 9: Requirements state outcomes, not steps
**FINDING-5** ⚠️ Important
- **GOAL-plat.4** reads as a procedure: "检测 → 提示确认 → 安装 → 启动 → 拉取模型 → 测试". This is a step-by-step implementation flow. The requirement should state: "After running init, the user has a working embedding provider with zero manual configuration steps beyond confirmation."
- **GOAL-plat.12** similarly describes a process: "读取 → 导入 → 移到 processed/". The requirement should be: "Files placed in the inbox directory are automatically ingested as memories and moved out of the inbox."
- **Suggestion**: Rewrite as outcome statements. The specific steps belong in design.

---

## Phase 3: Testability & Measurability

### Check 10: Pass/fail criteria are binary (unambiguous pass or fail)
**FINDING-6** ⚠️ Important
- **GOAL-plat.13**: "转录质量 ≥90% 准确率（英文/中文）" — How is "90% accuracy" measured? Word Error Rate (WER)? Character Error Rate? Against what reference transcript? This criterion is not practically testable without defining the measurement method and reference corpus.
- **Suggestion**: Either remove the accuracy threshold (STT quality depends on the engine, not KC) or specify: "WER ≤ 10% on a reference corpus of 10 test recordings" with the corpus defined.

### Check 11: Pass/fail criteria don't test implementation details
**FINDING-7** ⚠️ Important
- **GOAL-plat.2**: Pass/fail says `engram init` generates a config.toml and user edits it. This tests the specific config format (TOML) rather than the capability.
- **GOAL-plat.5**: Pass/fail checks specific log strings like `"embedding: ollama/nomic-embed-text"`. Log format is implementation detail.
- **Suggestion**: Rewrite pass/fail to test outcomes: "user can configure LLM provider and the system uses it" rather than testing specific file formats or log strings.

### Check 12: Quantitative criteria have units and thresholds
**FINDING-8** ⚠️ Minor
- **GOAL-plat.4**: "整个过程 < 3 分钟（不含下载时间）" — Good threshold, but "不含下载时间" makes it hard to measure in practice. The download IS part of the process from the user's perspective.
- **GOAL-plat.12**: "5 秒内文件被处理" — Good, measurable.
- **GOAL-plat.8**: "产生 ≥20 条记忆" — Good, measurable.
- Minor: consider whether the 3-minute threshold for plat.4 is testable as stated.

### Check 13: No "should" — only "must" or clear pass/fail
**✅ Pass** — No use of "should" or "ideally" in GOALs. All statements are definitive.

---

## Phase 4: Consistency & Conflicts

### Check 14: No contradictions between GOALs
**✅ Pass** — No contradictions found between the 14 GOALs. The fallback chain (plat.5) is consistent with graceful degradation (plat.3).

### Check 15: No contradictions with master GUARDs
**FINDING-9** ⚠️ Important
- **GUARD-3** (from master): "所有用户数据存储在本地，不发送到外部服务（除非用户 explicitly 配置云 LLM）"
- **GOAL-plat.10** (URL batch import): Fetches content from external URLs. This is technically "sending requests to external services." While it's user-initiated, the GUARD should either be clarified to exempt user-requested fetches, or GOAL-plat.10 should explicitly note this is user-initiated and doesn't violate the local-data principle.
- **GOAL-plat.14** (browser extension): Sends data from browser to local daemon — this is fine (local).
- **Suggestion**: Add a note to GOAL-plat.10 clarifying that URL fetching is user-initiated and data flows inward (external → local), not outward.

### Check 16: No duplicate/overlapping GOALs
**✅ Pass** — Each GOAL covers a distinct capability. GOAL-plat.8 (Markdown import) and GOAL-plat.9 (Obsidian import) are related but distinct (Obsidian adds wikilinks/frontmatter handling).

### Check 17: Priority assignments are consistent
**✅ Pass** — P0 for core infrastructure (LLM config, setup, import, intake), P1 for enhanced features (Obsidian, URL import, feature flags, degradation), P2 for nice-to-haves (bookmarks, browser extension). Logical and consistent.

### Check 18: Dependencies between GOALs are explicit
**FINDING-10** ⚠️ Important
- **GOAL-plat.9** (Obsidian import) depends on GOAL-plat.8 (Markdown import) — Obsidian is a superset of Markdown. This dependency is implicit.
- **GOAL-plat.10** (URL import) and GOAL-plat.11 (bookmarks import) both need HTTP fetching capability, but neither references the other.
- **GOAL-plat.5** (embedding fallback) depends on GOAL-plat.4 (Ollama setup) for its primary provider, but this isn't stated.
- **GOAL-plat.13** (voice intake) depends on GOAL-plat.12 (directory watch) for the inbox mechanism, but this isn't stated.
- **Suggestion**: Add a "Depends on" line to GOALs with dependencies: plat.9 → plat.8, plat.5 → plat.4, plat.13 → plat.12.

---

## Phase 5: Completeness & Coverage

### Check 19: Are there obvious missing requirements?
**FINDING-11** ⚠️ Important — Missing GOALs
- **Config migration/versioning**: What happens when config.toml schema changes between engram versions? No GOAL covers config migration or backward compatibility.
- **Import progress reporting**: For large imports (GOAL-plat.8, plat.10), there's no requirement for progress indication. A 1000-file Markdown import with no feedback is poor UX.
- **Import error handling**: GOAL-plat.10 mentions "失败的 URL 报告在输出中" but GOAL-plat.8 and plat.9 have no error handling requirements (what if a .md file is malformed?).
- **Suggestion**: Consider adding GOALs for config migration, import progress, and consistent error reporting across all import types.

### Check 20: Edge cases covered?
**FINDING-12** ⚠️ Minor
- **GOAL-plat.8**: What if Markdown files are very large (>1MB)? What about non-UTF-8 encoded files? Empty files?
- **GOAL-plat.12**: What if the inbox directory doesn't exist at daemon startup? What about file permission errors? Symlinks?
- **GOAL-plat.4**: What if `brew` is not installed on macOS? What about corporate machines with restricted install permissions?
- **Suggestion**: These are edge cases that could be addressed in pass/fail criteria or noted as "error handling: graceful failure with clear error message."

### Check 21: Non-functional requirements present where needed?
**FINDING-13** ⚠️ Minor
- No performance requirements for import operations (how fast should 1000 files import?).
- No storage overhead requirements (how much does import metadata cost per file?).
- These may be intentionally left to design, but for a P0 feature like batch import, basic throughput expectations help design decisions.
- **Suggestion**: Consider adding a performance expectation for GOAL-plat.8 (e.g., "1000 files in < 60 seconds excluding embedding generation").

### Check 22: Security considerations
**✅ Pass** — GOAL-plat.2 handles API keys in config. Master GUARD-4 covers secret leakage. GOAL-plat.14 (browser extension) communicates with localhost only. URL fetching (plat.10) is user-initiated.

### Check 23: Backward compatibility addressed?
**✅ Pass for this scope** — This is a new feature (KC), so backward compatibility with existing engram API is handled by GOAL-plat.7 (feature flags). Existing `engram` users without KC features shouldn't be affected.

### Check 24: Are out-of-scope items listed?
**FINDING-3 addendum** ✅ Applied (added Out of Scope section) — The document has no explicit "Out of Scope" section. While the master doc may cover this, each feature doc benefits from listing what's explicitly NOT covered (e.g., "cloud-hosted KC is out of scope", "mobile app intake is out of scope").

### Check 25: GOAL count appropriate for feature scope?
**✅ Pass** — 14 GOALs for a feature covering LLM config + setup + import + intake is appropriate. The feature could be split further (LLM config vs import vs intake), but at 14 GOALs it's within the ≤15 limit.

### Check 26: Master GUARD coverage
**✅ Pass (partial)** — The overview correctly references GUARD-1 through GUARD-6. However, as noted in FINDING-2, individual GOALs don't map to specific GUARDs. Checking each GUARD:
- **GUARD-1** (100% offline capable): Covered by GOAL-plat.3 (graceful degradation) and plat.5 (embedding fallback)
- **GUARD-2** (engram recall API 不变): Covered by GOAL-plat.7 (feature flags preserve API)
- **GUARD-3** (local data): Partially — see FINDING-9 re: URL fetching
- **GUARD-4** (no secret leakage): Implicitly covered by config handling, should be explicit
- **GUARD-5** (< 500ms recall): Not directly relevant to platform setup GOALs
- **GUARD-6** (no silent data loss): Relevant to import/intake GOALs but not explicitly referenced

---

## Findings Summary

| ID | Severity | Phase | GOAL(s) | Summary |
|----|----------|-------|---------|---------|
| FINDING-1 | Minor | 1 | — | §5.5 half-step numbering — verify alignment with master ✅ Applied |
| FINDING-2 | Important | 1 | All | No per-GOAL GUARD mapping ✅ Applied |
| FINDING-3 | Critical | 2 | plat.4, plat.6 | Implementation leakage: specific install commands and distribution channels ✅ Applied |
| FINDING-4 | Critical | 2 | plat.2, plat.7 | Implementation leakage: config schema fields and Cargo feature flag names ✅ Applied |
| FINDING-5 | Important | 2 | plat.4, plat.12 | Procedural steps instead of outcome statements ✅ Applied |
| FINDING-6 | Important | 3 | plat.13 | Unmeasurable "90% accuracy" criterion ✅ Applied |
| FINDING-7 | Important | 3 | plat.2, plat.5 | Pass/fail tests implementation details (TOML format, log strings) ✅ Applied |
| FINDING-8 | Minor | 3 | plat.4 | "不含下载时间" makes threshold hard to measure ✅ Applied |
| FINDING-9 | Important | 4 | plat.10 | URL fetching may conflict with GUARD-3 (local data) ✅ Applied |
| FINDING-10 | Important | 4 | plat.5/9/13 | Implicit inter-GOAL dependencies not stated ✅ Applied |
| FINDING-11 | Critical | 5 | — | Missing GOALs: config migration, import progress, error handling ✅ Applied (added GOAL-plat.15, GOAL-plat.16, error handling to plat.8/9/12) |
| FINDING-12 | Minor | 5 | plat.4/8/12 | Edge cases not addressed (large files, permissions, missing tools) ✅ Applied (added Error handling blocks to plat.4/8/12) |
| FINDING-13 | Minor | 5 | plat.8 | No performance requirements for import operations ✅ Applied (deferred to design; GOAL-plat.15 covers reporting) |

---

## Recommendation

Address the 3 critical findings before proceeding to design:

1. **FINDING-3 + FINDING-4**: Extract implementation details (install commands, config field names, feature flag names) from GOALs into design doc. Rewrite GOALs as outcome statements. ✅ Applied
2. **FINDING-11**: Add missing GOALs for config migration, import progress reporting, and consistent error handling across import types. ✅ Applied (GOAL-plat.15 + GOAL-plat.16 added)

The 6 important findings (FINDING-2, 5, 6, 7, 9, 10) should ideally be addressed but are not blockers for design phase. ✅ All applied
