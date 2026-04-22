# Review: maintenance/requirements.md (Round 1)

**Review depth**: standard (Phases 0–5, 26 checks)
**Date**: 2026-04-17
**Document**: `.gid/features/knowledge-compiler/maintenance/requirements.md`
**Master GUARDs**: `.gid/features/knowledge-compiler/requirements.md` (GUARD-1 through GUARD-6)
**Reviewer**: RustClaw (autopilot)

---

## 🔴 Critical (blocks implementation)

### FINDING-1 — [Check #6] GOAL-maint.6: Implementation leakage in recall ranking ✅ Applied
GOAL-maint.6 specifies "topic page 出现在结果 top 3 中，且排在相关碎片记忆之前" — this dictates a specific ranking algorithm (topic pages always beat fragment memories). The WHAT should be "compiled knowledge is surfaced when relevant" — the HOW of ranking position is a design decision.

**Substitution test**: Could someone satisfy this with a different ranking strategy (e.g., topic pages get a relevance boost factor rather than hard "always top 3")? No — the requirement locks in a specific ranking behavior.

**Suggested fix**: Rewrite pass/fail to: "recall("Rust async patterns") 时，如果存在相关 topic page，该 page 出现在结果中且排序权重高于等价的碎片记忆。具体排序策略由 design 决定。"

### FINDING-2 — [Check #6] GOAL-maint.8: Implementation leakage — specifies exact CLI subcommand names and signatures ✅ Applied
GOAL-maint.8 lists exact CLI subcommand names (`engram compile`, `engram topics`, `engram topic <id>`, `engram kb-health`, `engram kb-export <path>`). These are interface design decisions, not requirements. The requirement is "user can trigger compilation, list topics, view a topic, check health, and export via CLI."

**Substitution test**: Could someone use `engram kc compile` or `engram knowledge compile` instead? No — the requirement pins exact names.

**Suggested fix**: Rewrite to: "engram CLI 提供 Knowledge Compiler 子命令，覆盖以下操作：触发编译周期、列出所有 topic pages（含活跃度和状态）、查看单个 topic page 内容、运行健康检查、导出为 Markdown。具体命令名和参数格式由 design 决定。Pass/fail: 每个操作可通过 CLI 完成且输出人类可读。"

### FINDING-3 — [Check #6] GOAL-maint.9: Implementation leakage — specifies exact Rust method signatures ✅ Applied
GOAL-maint.9 specifies `memory.compile()`, `memory.topics()`, `memory.topic("id")`, `memory.kb_export("path")` — these are API design decisions. The requirement should be "programmatic access to all KC operations."

**Substitution test**: Could someone put these on a `KnowledgeCompiler` struct instead of `Memory`? Or use `memory.knowledge().compile()`? No — method names and struct placement are locked in.

**Suggested fix**: Rewrite to: "engram crate 暴露 Knowledge Compiler 的 Rust API，agent 和应用可以程序化调用所有 GOAL-maint.8 中描述的操作。API 设计由 design 决定。Pass/fail: 通过 Rust API 可完成编译、列出 topics、查看单个 topic、健康检查、导出的所有操作。"

### FINDING-4 — [Check #6] GOAL-maint.12: Implementation leakage — specifies SQLCipher and env var name ✅ Applied
GOAL-maint.12 specifies "SQLCipher 或等效方案" and the exact env var `ENGRAM_DB_KEY`. The requirement should be "optional at-rest encryption for the knowledge database."

**Suggested fix**: Rewrite to: "支持可选的数据库 at-rest 加密。加密密钥通过安全渠道获取（非明文配置文件）。Pass/fail: 启用加密后，DB 文件无法被未授权工具直接读取。密钥获取方式由 design 决定。"

---

## 🟡 Important (should fix before implementation)

### FINDING-5 — [Check #1] GOAL-maint.1: Vague decay threshold for "archived" ✅ Applied
GOAL-maint.1 says "极低活跃度 pages 标记为 archived" — what is "极低"? The pass/fail uses ">50% drop in 30 days" for the decay itself (good), but never defines the archived threshold. An implementer would have to guess.

**Suggested fix**: Add to pass/fail: "活跃度评分低于可配置阈值（默认值由 design 定义）时标记为 archived。"

### FINDING-6 — [Check #3] GOAL-maint.4: Hard-coded similarity threshold ✅ Applied
GOAL-maint.4 specifies "相似度 >0.85" — this is a concrete number (good for testability) but should be configurable since optimal thresholds depend on embedding model and domain. Consider: "相似度超过可配置阈值（默认 0.85）".

**Suggested fix**: Change to "系统检测到相似度超过可配置阈值（默认 0.85）并标记为疑似重复。"

### FINDING-7 — [Check #8] Missing error/edge case: LLM failure during conflict detection ✅ Applied
GOAL-maint.2 (conflict detection) relies on synthesis engine + 语义比对, which likely requires LLM. What happens when LLM is unavailable during a compile cycle? The requirement doesn't address graceful degradation. GUARD-6 says browsing/search don't need LLM, but compilation does — what about the maintenance operations that straddle both?

**Suggested fix**: Add a note or a new GOAL: "LLM 不可用时，conflict detection 和 duplicate detection 降级为 embedding-only 比对（精度降低但不阻塞维护流程），并在 health report 中标记 'degraded mode: LLM unavailable'."

### FINDING-8 — [Check #8] Missing error/edge case: empty knowledge base ✅ Applied
Multiple GOALs assume an existing knowledge base (GOAL-maint.5 health report, GOAL-maint.7 export, GOAL-maint.6 recall). What happens when there are 0 topic pages? Is `kb-health` expected to return zeros gracefully, or error out?

**Suggested fix**: Add to GOAL-maint.5 pass/fail: "对空知识库运行健康检查，输出 total pages = 0 且不报错。" Similarly, GOAL-maint.7 export on empty KB should produce an empty folder or a single index file, not crash.

### FINDING-9 — [Check #9] Missing non-functional: performance requirements for maintenance operations ✅ Applied (deferred to design — GOAL-maint.5b added for observability, specific performance numbers left to design)
No GOAL specifies performance bounds for maintenance operations. How long should a health check take on a 1000-page KB? How long should export take? Conflict detection on 100 new memories? Without these, an implementation that takes 10 minutes for health check technically passes.

**Suggested fix**: Add a performance GOAL or note: "health check 完成时间 < 5s（1000 pages），export < 30s（1000 pages），conflict detection per memory < 2s。具体数字由 design 确定，但量级要求明确。"

### FINDING-10 — [Check #9] Missing non-functional: observability for maintenance operations ✅ Applied (added GOAL-maint.5b)
No requirement for logging/metrics of maintenance operations. When compile runs in RustClaw's heartbeat, how does the user know what happened? GOAL-maint.11 covers LLM transparency but not general operation logging (e.g., "compiled 3 pages, found 1 conflict, repaired 0 links").

**Suggested fix**: Add a GOAL or expand GOAL-maint.5: "每次维护操作（compile、maintenance cycle）输出操作摘要：编译了多少 pages、检测到多少冲突/broken links/duplicates、耗时、LLM token cost。"

### FINDING-11 — [Check #12] Potential contradiction: GOAL-maint.6 vs GUARD-6 ✅ Applied (clarified GUARD-6 in master requirements)
GUARD-6 says "知识库的浏览和搜索不依赖 LLM." GOAL-maint.6 says topic pages participate in recall ranking. If topic pages were compiled by LLM, and recall surfaces them, the recall result depends on LLM-generated content. This is arguably fine (the *search* doesn't call LLM, it just surfaces pre-compiled content), but the boundary is ambiguous. Clarify that GUARD-6 means "no LLM call at query time," not "results don't contain LLM-generated content."

**Suggested fix**: In master requirements, clarify GUARD-6: "浏览和搜索操作本身不调用 LLM（query-time 无 LLM 依赖）。搜索结果可包含 LLM 预编译的 topic page 内容。"

### FINDING-12 — [Check #14] Priority inversion: GOAL-maint.9 [P1] depends on GOAL-maint.8 [P0] ✅ Applied (added Dependencies section)
GOAL-maint.9 (Programmatic API, P1) is defined as covering "GOAL-maint.8 的所有操作." If the API exactly mirrors CLI operations, the API design is blocked by CLI design. This is fine since P0 > P1, but the dependency should be explicit.

More critically: GOAL-maint.7 (Markdown Export, P0) and GOAL-maint.6 (Knowledge-Aware Recall, P0) have no explicit dependencies on compilation GOALs. Export needs topic pages to exist — which requires GOAL-comp.* to be implemented first. This cross-feature dependency should be stated.

**Suggested fix**: Add a "Dependencies" note at the bottom: "GOAL-maint.6, maint.7, maint.8 depend on compilation feature (GOAL-comp.1–5) producing topic pages. GOAL-maint.9 mirrors GOAL-maint.8."

---

## 🟢 Minor (can fix during implementation)

### FINDING-13 — [Check #4] GOAL-maint.8: Compound requirement (5 subcommands in one GOAL) ✅ Applied (added all-must-pass clause)
GOAL-maint.8 bundles 5 CLI subcommands into one GOAL. Each could independently pass or fail. If `compile` works but `kb-export` doesn't, does GOAL-maint.8 pass or fail? Consider splitting into GOAL-maint.8a–8e, or accept that it's a "CLI surface" meta-requirement and note that ALL subcommands must pass.

**Suggested fix**: Add: "所有子命令全部通过才视为 pass。任一子命令失败即 fail。" Or split into individual GOALs per subcommand.

### FINDING-14 — [Check #13] Terminology: "archived" vs "stale" vs "inactive" ✅ Applied (added Terminology table)
GOAL-maint.1 uses "archived" for low-activity pages. The master requirements' §4 mentions "stale 标记" and "archived 虚线" as distinct visual states in the Web UI. Are "stale" and "archived" the same state, or different points on a decay curve? The compilation requirements use "stale" to mean "needs recompilation," which is a different concept entirely.

**Suggested fix**: Define terminology explicitly: "stale = needs recompilation (content outdated), archived = low activity (still valid, just rarely accessed)." Add a terminology table to master requirements.

### FINDING-15 — [Check #22] Numbering gap awareness ✅ Applied (added section numbering note)
GOALs go maint.1–12 with no gaps. Clean. But the section numbering jumps from §2 to §3 to §8, following the master document's numbering scheme. This is fine for cross-referencing consistency, but worth noting: if someone reads this document standalone, §4–§7 missing could be confusing.

**Suggested fix**: Add a note at the top: "Section numbers (§2, §3, §8) follow master requirements numbering. Sections §1, §4–§7 are in other feature documents."

### FINDING-16 — [Check #5] GOAL-maint.3: Missing trigger/actor specification ✅ Applied
GOAL-maint.3 says "运行维护" but doesn't specify who triggers it. Is it automatic (heartbeat), manual (`engram maintain`), or both? The pass/fail assumes manual ("删除一条记忆后，运行维护"), but GOAL-maint.8 doesn't list a `maintain` subcommand.

**Suggested fix**: Clarify: "维护操作作为 compile 周期的一部分自动运行，也可通过 CLI 手动触发（具体命令由 GOAL-maint.8 定义）。"

### FINDING-17 — [Check #23] Section grouping spans disparate concerns ✅ Applied (noted, no change needed — under 15 GOALs)
§2 (Maintenance), §3 (Access & Export), and §8 (Privacy) are three different domains bundled in one feature doc. This works for a 12-GOAL document (under the 15 limit), but if the feature grows, it should be split. Currently acceptable.

**No change needed** — just flagging for future awareness.

---

## 📊 Coverage Matrix

| Category | Covered | Missing/Weak |
|---|---|---|
| Happy path (maintenance) | GOAL-maint.1,2,3,4,5 | ✅ All maintenance ops covered |
| Happy path (access) | GOAL-maint.6,7,8,9 | ✅ All access patterns covered |
| Happy path (privacy) | GOAL-maint.10,11,12 | ✅ Three-tier privacy covered |
| Error handling | GOAL-maint.3 (broken links) | ⚠️ LLM failure during maintenance (FINDING-7), empty KB (FINDING-8) |
| Performance | — | ⚠️ No performance bounds for any maintenance op (FINDING-9) |
| Security | GOAL-maint.10,11,12 | ✅ Local-first + transparency + optional encryption |
| Observability | GOAL-maint.5 (health report), maint.11 (LLM verbose) | ⚠️ No per-operation logging/metrics (FINDING-10) |
| Scalability | — | ⚠️ No mention of behavior at scale (10k+ pages, 100k+ memories) |
| Edge cases | — | ⚠️ Empty KB (FINDING-8), concurrent maintenance runs not addressed |

---

## ✅ Passed Checks

- **Check #0**: Document size ✅ — 12 GOALs, under the 15-GOAL limit. No split needed.
- **Check #1**: Specificity ✅ (10/12 GOALs specific; GOAL-maint.1 "极低活跃度" flagged in FINDING-5, rest are concrete)
- **Check #2**: Testability ✅ — All 12 GOALs have explicit pass/fail criteria. Each is testable.
- **Check #3**: Measurability ✅ (11/12; GOAL-maint.4 threshold flagged as hard-coded in FINDING-6, but it IS measurable)
- **Check #4**: Atomicity ✅ (11/12; GOAL-maint.8 is compound but acceptable — see FINDING-13)
- **Check #5**: Completeness ✅ (11/12; GOAL-maint.3 missing trigger actor — FINDING-16)
- **Check #6**: Implementation leakage — ❌ 4 findings (FINDING-1,2,3,4). Most significant issue in this document.
- **Check #7**: Happy path coverage ✅ — All three domains (maintenance, access, privacy) have complete happy paths.
- **Check #8**: Error/edge case coverage — ⚠️ 2 gaps found (FINDING-7, FINDING-8)
- **Check #9**: Non-functional requirements — ⚠️ 2 gaps (FINDING-9 performance, FINDING-10 observability)
- **Check #10**: Boundary conditions ✅ — GOAL-maint.1 has concrete 30-day/50% bounds; GOAL-maint.4 has 0.85 threshold; GOAL-maint.5 has 10+ pages test. No unbounded numerics.
- **Check #11**: State transitions ✅ — Page lifecycle states (active → stale → archived) are implied in GOAL-maint.1. Could be more explicit, but covered.
- **Check #12**: Internal consistency — ⚠️ 1 ambiguity (FINDING-11, GUARD-6 vs GOAL-maint.6)
- **Check #13**: Terminology consistency — ⚠️ 1 issue (FINDING-14, stale vs archived)
- **Check #14**: Priority consistency — ⚠️ 1 issue (FINDING-12, cross-feature dependency)
- **Check #15**: Numbering/referencing ✅ — All cross-references resolve. Master GUARD references valid.
- **Check #16**: GUARDs vs GOALs alignment ✅ — No GOAL violates any GUARD. GUARD-5 (non-destructive) compatible with all maintenance GOALs. GUARD-1 (engram-native) compatible with CLI/API GOALs.
- **Check #17**: Technology assumptions ✅ — LLM dependency is explicit and justified. SQLite is from GUARD-1. No hidden tech assumptions.
- **Check #18**: External dependencies ✅ — LLM provider dependency stated. Synthesis engine dependency stated. Obsidian compatibility is a format spec, not a runtime dependency.
- **Check #19**: Data requirements ✅ — Source data (engram memories) well-defined. Output formats (Markdown, wikilinks) specified.
- **Check #20**: Migration/compatibility ✅ — This is new functionality, no migration needed. GOAL-maint.9 API is additive to existing `Memory` struct.
- **Check #21**: Scope boundaries ✅ — Master doc has explicit "Out of Scope" section covering multi-user, multi-device sync, real-time collaboration, Obsidian bidirectional sync, Web UI, marketplace, image/video, custom LLM fine-tuning.
- **Check #22**: Unique identifiers ✅ — GOAL-maint.1 through GOAL-maint.12, no gaps, no duplicates.
- **Check #23**: Grouping ✅ — Organized by domain (§2 maintenance, §3 access, §8 privacy). Logical grouping. See FINDING-17 for future awareness.
- **Check #24**: Dependency graph — ⚠️ Cross-feature dependencies not explicit (FINDING-12). Internal dependencies implicit but logical (health report depends on all maintenance features existing).
- **Check #25**: Acceptance criteria ✅ — Every GOAL has a dedicated pass/fail block. All are concrete and testable independently.

---

## Summary

- **Total requirements**: 12 GOALs + 6 GUARDs (from master)
- **Critical**: 4 (all implementation leakage — FINDING-1,2,3,4)
- **Important**: 8 (FINDING-5 through FINDING-12)
- **Minor**: 5 (FINDING-13 through FINDING-17)
- **Total findings**: 17
- **Coverage gaps**: Performance bounds, observability/logging, LLM failure degradation, empty KB edge case
- **Recommendation**: Needs fixes first — the 4 critical implementation leakage findings should be resolved before design begins. The requirements are otherwise well-structured with strong testability.
- **Estimated implementation clarity**: Medium-High (clear what to build, but some edge cases and cross-feature dependencies need documenting)
