# Feature: Dimensional Memory Extraction

**Date**: 2026-04-21
**Status**: Approved
**Scope**: engram extract + storage + type inference

---

## 1. Problem Statement

当前 engram 的 memory extraction 有三个结构性问题：

### 1.1 信息丢失（extract 层）
LLM extractor 把原始对话压缩成一个 `content` 字符串，语义维度（谁、什么时候、为什么、结果如何）全靠 LLM 自由发挥塞进去。实际效果是大量泛化——"昨天 potato 因为 Python 太慢决定换 Rust" 变成 "User prefers Rust for development"，时间、人名、因果关系全丢。

### 1.2 memory_type 强制单选
`memory_type: MemoryType` 是一个枚举，一条记忆只能选一个 type。但真实记忆经常同时具备多种属性（episodic + causal + factual）。LLM 被迫选一个，选错了 recall 时 type affinity 加不上去。

### 1.3 无 raw 保底
extract 成功后原文被丢弃。只有 extract **失败**时才 fallback 存原文。如果 LLM 泛化或遗漏了信息，永久丢失，无法事后重新 extract。

---

## 2. Goals

### GOAL-1: 多维度结构化抽取
Extract 层将每条记忆分解为多个独立语义维度，覆盖 5W2H（who/what/when/where/why/how + outcome）及关系信息：

- **核心事实**（必填）：发生了什么
- **参与者**：涉及的人或实体
- **时间**：何时发生
- **地点/来源**：在哪里发生（物理地点、聊天频道、项目上下文等）
- **背景/情境**：发生时的上下文
- **原因/动机**：为什么发生
- **结果/影响**：导致了什么
- **方法/步骤**：如何做的
- **关系**：与其他已知事物的关联

每个维度独立存储，非必填维度可为空。维度通过 LLM structured output 填充。

具体字段名、类型定义、存储格式见 design.md。

### GOAL-2: memory_type 多值推导
memory_type 从 LLM 判断的单选枚举改为由维度填充情况自动推导的多值权重向量。

- 不再要求 LLM 选择 memory_type
- 每种 type（factual/episodic/procedural/relational/emotional/opinion/causal）各有一个 0-1 权重
- 权重由维度填充组合决定（规则式推导，不额外调 LLM）
- 现有 recall 中的 type affinity 乘数从离散匹配改为与权重向量的连续相乘

具体推导规则和数据结构见 design.md。

### GOAL-3: Raw 层存储
每次 extract 成功后，原始文本以 metadata 字段 `source_text` 存储在第一条 extracted fact 上，不丢弃。

- 不参与 embedding 计算
- 不参与 recall 排序
- 不增加额外记录数
- 可通过 metadata 查询取回，用于事后重新 extract（如 prompt 改进后批量重跑）

### GOAL-4: 向后兼容
- 现有记忆数据不能丢失
- SQLite schema 变更需要 migration
- 旧记忆在 recall 时仍能正常工作（旧 `content` 映射到核心事实维度，其余维度为空，type 权重设为默认值）
- `add()`/`recall()` 公共方法签名保持兼容
- `ExtractedFact` struct 字段变更属于 breaking change，通过 semver minor bump 处理（0.x 语义下 minor = breaking is acceptable）
- 用户如果实现了自定义 `MemoryExtractor`，需要适配新 `ExtractedFact`——在 CHANGELOG 中明确说明

---

## 3. Non-Goals

- **不改 recall ranking 算法** — 先改 extract，跑 benchmark 看效果，再决定 recall 要不要加 dimensional matching channel
- **不改 embedding 策略** — 对什么字段算 embedding 是后续问题，本次用核心事实维度或拼接非空维度
- **不做 NER/regex 抽取** — 维度全用 LLM structured output，不引入规则式 pipeline
- **不改 ISS-018 (intent classification)** — 那是 recall 层的改动，跟这里正交
- **不做 structured output 降级策略** — v1 仅支持通过 LLM structured output（tool_use/function calling）抽取。不支持 structured output 的 provider（如部分 Ollama 本地模型）将 fallback 到现有行为（存 raw content）。完整的 prompt-based JSON fallback 是后续工作。

---

## 4. Guards

### GUARD-1: Extract 成本可控
维度抽取仍然是 1 次 LLM call（跟现在一样），不加额外 call。通过 structured output 约束格式，不是两步 extract。单次 response token 大致持平或略增 ~10-15%（多字段 JSON key 开销 vs 单字符串，但每个字段更精炼不用写完整句），属可接受范围。

### GUARD-2: 无信息退化
改动后存储的信息量 ≥ 改动前。即使 LLM 某个维度没填，至少核心事实维度等价于现有 `content`。

### GUARD-3: Recall 性能不退化
type affinity 从离散匹配改为连续权重，计算成本 O(7) 乘法，可忽略。不引入新的 recall 延迟。

### GUARD-4: Migration 安全
SQLite schema 变更必须有 migration，旧数据不丢失。旧记忆的 `content` 映射到核心事实维度，其余维度为 null，type 权重全部设为默认值。

### GUARD-5: Dedup 兼容
改动后 embedding 基于核心事实维度（可能比原 `content` 更短更精炼），dedup 相似度阈值可能需要调整。实施时需验证 dedup 行为没有退化（不误合并、不漏合并）。

---

## 5. Success Criteria

1. 新 extract 的记忆在核心事实之外至少有 1 个维度非空（覆盖率 > 80%）
2. memory_type 推导验证：选 50 条现有记忆，人工标注多值 type ground truth，推导规则一致率 > 90%
3. 原文在 extract 成功后可通过 metadata 查询取回
4. 所有现有 247 tests 通过（或合理更新）
5. Recall benchmark 准确率不低于改动前（如果有提升更好，但不是本次目标）
