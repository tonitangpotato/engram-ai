# Requirements Review — dimensional-extract r1

**Date**: 2026-04-21
**Reviewer**: RustClaw
**Depth**: standard (Phase 0-5)
**Document**: `.gid/features/dimensional-extract/requirements.md`

---

## Summary

- **Critical**: 1
- **Important**: 4
- **Minor**: 2
- **Suggestion**: 2

Total: 9 findings

---

## Findings

### FINDING-1 — Important: Implementation leakage in GOAL-1 ✅ Applied

GOAL-1 定义了 8 个具体字段名（what/who/when/context/why/outcome/how/relation）和它们的类型（String/null）。这些是设计决策，不是需求。

需求应该是："extract 层将每条记忆分解为多个语义维度，覆盖 5W1H（who/what/when/where/why/how）及因果关系，每个维度独立存储"。具体字段名、是 8 个还是 6 个、字段是 String 还是 enum——这些是 DESIGN.md 的事。

**建议**: 把维度表移到设计文档，GOAL-1 只保留语义要求："extract 输出至少覆盖：核心事实、参与者、时间、背景、因果、方法"。

### FINDING-2 — Important: Implementation leakage in GOAL-2 ✅ Applied

`TypeScores` struct 定义（含 Rust 字段名和 f32 类型）是纯实现细节。推导规则（"when 非空 + context 非空 → episodic 分高"）也是设计层的。

需求应该是："memory_type 从单值分类改为多值权重向量，由抽取维度的填充情况自动推导，不由 LLM 判断"。

**建议**: struct 定义和推导规则移到 design.md。

### FINDING-3 — Critical: GOAL-3 raw 存储方案模糊，两个选项未决 ✅ Applied

GOAL-3 给了两个方案（metadata 字段 vs 独立记录）但没选。两个方案在 schema、recall 行为、存储开销上完全不同：

- **metadata 字段**: 不增加记录数，但 recall 时 raw 文本不会被搜到（因为 embedding 是算在 what 上的）
- **独立记录**: 记录数翻倍，recall 时可能把 raw 记录也搜出来（需要过滤逻辑）

"方案 A 或方案 B" 不是需求，是未完成的决策。

**建议**: 选一个。如果目的是"事后可重新 extract"，metadata 字段就够了（不需要被 recall 搜到，只需要能 query 出来）。明确写："原文存储为 extracted fact 的 metadata 字段 `source_text`，不参与 embedding 或 recall 排序"。

### FINDING-4 — Important: 缺少 structured output 降级策略 ✅ Applied

GOAL-1 说"LLM 通过 structured output (tool_use / function calling) 填充"。但 structured output 支持取决于 LLM provider：
- Anthropic tool_use: 支持
- OpenAI function calling: 支持
- Ollama 本地模型: 大部分不支持 structured output

engram 作为通用 crate（published on crates.io），不能假设所有用户都用 Anthropic。当 provider 不支持 structured output 时，fallback 是什么？

**建议**: 加一条需求："当 LLM provider 不支持 structured output 时，fallback 到 prompt-based JSON extraction + 解析验证"。或者明确声明 non-goal："structured output 降级策略不在本次范围，v1 仅支持 Anthropic tool_use"。

### FINDING-5 — Important: GOAL-4 "add() 公共 API 保持兼容" 与 GOAL-1 矛盾 ✅ Applied

GOAL-1 要改 `ExtractedFact` 从 `content: String` 变成 8 维度字段。`ExtractedFact` 是 pub struct（在 extractor.rs），改字段就是 breaking change。

GOAL-4 说"add() 公共 API 保持兼容"——`add()` 方法签名或许不变，但 `ExtractedFact` 是 MemoryExtractor trait 的输出类型，用户如果实现了 `MemoryExtractor` 或直接构造 `ExtractedFact`，就会 break。

**建议**: 明确兼容范围——是 "add()/recall() 签名不变" 还是 "ExtractedFact 也不能改字段"？如果允许 breaking change（semver minor/major），直说。如果不允许，需要用 extension 方式（新字段加到 metadata 或新 struct）。

### FINDING-6 — Minor: GUARD-1 假设 structured output 不增加 token 成本 ✅ Applied

"1 次 LLM call，不加额外 call"——对的，call 数不增加。但 structured output 的 response token 数很可能增加（8 个字段 vs 1 个 content 字符串），特别是大部分维度非空时。

这不是 blocker，但应该明确："call 数不增加，单次 response token 可能增加 ~30-50%，属可接受范围"。

### FINDING-7 — Minor: Success Criteria #2 验证方法不具体 ✅ Applied

"memory_type 推导与人工判断一致率 > 90%（用现有测试数据验证）"——现有测试数据是什么？有标注了 ground truth memory_type 的数据集吗？如果没有，这个 criteria 没法验证。

**建议**: 要么指定数据源，要么改成 "选 50 条现有记忆，人工标注多值 type，验证推导规则一致率"。

### FINDING-8 — Suggestion: 缺少 dedup 影响分析 ✅ Applied

当前 dedup 用 embedding 余弦相似度。改成 8 维度后，embedding 算在 `what` 字段上（按 non-goal 所述）。如果 `what` 比原来的 `content` 更短更精炼，dedup 的相似度阈值可能需要调整。

不是 blocker（dedup 调阈值很简单），但应该提一嘴。

### FINDING-9 — Suggestion: 考虑是否需要 `where` 维度 ✅ Applied

8 维度覆盖了 5W1H 中的 who/what/when/why/how，但没有 where。用 `context` 隐含了 where，但 context 更多是"情境"不是"地点"。对于有空间信息的记忆（"在办公室讨论的"、"在 Discord 群里说的"），where 有独立价值。

可能不需要，但值得有意识地决定——是合并在 context 里，还是单独加一个 where。

---

## Applied Status

- **FINDING-1** ✅ Applied — GOAL-1 移除字段名/类型表，改为语义维度描述，引用 design.md
- **FINDING-2** ✅ Applied — GOAL-2 移除 TypeScores struct 和推导规则，只留语义要求
- **FINDING-3** ✅ Applied — GOAL-3 选定 metadata `source_text` 方案，明确不参与 embedding/recall
- **FINDING-4** ✅ Applied — Non-Goals 新增 structured output 降级策略说明（v1 不做，fallback 到 raw）
- **FINDING-5** ✅ Applied — GOAL-4 明确 ExtractedFact 是 breaking change，semver 0.x minor bump，CHANGELOG 说明
- **FINDING-6** ✅ Applied — GUARD-1 补充 token 成本说明（~30-50% 增加，可接受）
- **FINDING-7** ✅ Applied — Success Criteria #2 改为"选 50 条人工标注验证"
- **FINDING-8** ✅ Applied — 新增 GUARD-5: Dedup 兼容性检查
- **FINDING-9** ✅ Applied — GOAL-1 维度列表增加"地点/来源"（where），从 8 维度变 9 维度

## Overall Assessment

All 9 findings applied. 文档已从 draft 升级为 review-ready。
