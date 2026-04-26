# ISS-021 Sample Audit — 10 条维度空记忆人工标注

**Date**: 2026-04-22
**Source**: `.gid/issues/ISS-019-dimensional-metadata-write-gap/pilot/target-100.db`
**Purpose**: 量化 H4（内容真稀疏）vs H2（有信息但没抽出）的比例

## 标注规则

对每条"维度为空"的记忆，判断**原文是否包含 participants / temporal / causation 信息**：

- **✅ HAS**: 原文明确提到
- **🟡 IMPLICIT**: 可从上下文推断但原文没明写
- **❌ NONE**: 原文真的没有该维度信息

## 10 条样本标注

| # | ID | Content (truncated) | participants | temporal | causation | 判定 |
|---|---|---|---|---|---|---|
| 1 | 460dfecf | Max aggregation penalizes new memories with lower scores (0.5) compared to old memories (1.0) under neutral queries | ❌ | 🟡 ("new" vs "old"不是具体时间) | ✅ ("penalizes... under neutral queries" 因→果) | causation 本该抽 |
| 2 | 88227c3d | LocomoAdapter has been written and Locomo dataset is present locally | ❌ | ❌ ("has been" 只是完成态) | ❌ | 真稀疏 |
| 3 | f588fc8b | KC-Auto-Recall requirements completed with 11 GOALs and 6 GUARDs, documented in .gid/features/kc-auto-recall/requirements.md | ❌ | ❌ | ❌ | 真稀疏 |
| 4 | f7474486 | Task document directory for ISS-019-... is located at /... | ❌ | ❌ | ❌ | 真稀疏 |
| 5 | bebc789e | Two memory system improvements identified but not yet implemented: (1) Memory-type-aware retrieval..., (2) Spreading activation... | ❌ | 🟡 ("not yet implemented" = 未完成态) | ❌ | 真稀疏 |
| 6 | f098185f | Hacker News viral post failure modes include: over-marketing, suspicious benchmarks... | ❌ | ❌ | ✅ ("failure modes include X, Y" 因果结构) | causation 本该抽 |
| 7 | d3bcb096 | Recommended architecture uses Regex as L1 with 78% accuracy improving to 90% after optimization, plus Haiku API as L2 parallel fallback for remaining 10% | ❌ | ❌ | ✅ ("after optimization" → 改进原因)🟡 | 边缘 |
| 8 | 9041ee4d | Step 1 of §11 implementation plan involves type definitions in src/dimensions.rs — lowest-risk entry point with zero behavioral changes | ❌ | ❌ | ✅ ("lowest-risk... because zero behavioral changes" 因果)🟡 | 边缘 |
| 9 | 96730769 | Memory::new() method lacks auth tokens required for classifier construction; needs a separate auto_configure_intent_classifier() method | ❌ | ❌ | ✅ ("lacks X, needs Y" = 因为缺 X 所以要 Y) | causation 本该抽 |
| 10 | 8a641fdc | Sequential processing adds 200ms (12% overhead) to query latency, p95 reaching 500ms, necessitating parallelization | ❌ | ❌ | ✅ ("necessitating" = 因此需要) | causation 本该抽 |

## 统计

**Participants**:
- HAS: 0/10 (0%)
- IMPLICIT: 0/10
- NONE: 10/10 (100%)
→ **H4 主导**：文档性内容本就没有人物角色

**Temporal**:
- HAS: 0/10 (0%)
- IMPLICIT: 2/10 ("new/old", "not yet") — 不算真正时间
- NONE: 8/10 (80%)
→ **H4 主导**：抽象概念无时间戳

**Causation**:
- HAS: 4/10 明确因果 (460dfecf, f098185f, 96730769, 8a641fdc)
- IMPLICIT (边缘): 2/10 (d3bcb096, 9041ee4d)
- NONE: 4/10 (88227c3d, f588fc8b, f7474486, bebc789e)
→ **H2 有一定比例**：至少 40% 的条目原文有明确因果但没抽出来

## 初步结论

1. **participants / temporal**: H4 主导（内容真稀疏），**不是 bug**，改 extractor 收益低
2. **causation**: H2 有实质比例（~40% miss rate），**改 extractor prompt 有可观收益**

## 对方向 A/B/C 的含义

- **方向 A**（保留 extractor，ranking 容忍缺失）：
  - participants / temporal 缺失当中性 → 合理（内容真稀疏）
  - causation 缺失当中性 → **浪费 40% 应抽而未抽的信号**
  
- **方向 B**（改 extractor prompt 允许推断）：
  - 只改 causation 部分收益最高
  - participants / temporal 强行推断 → 污染（会强行填不准的值）
  
- **方向 C**（双 extractor：保守抽取 + 查询侧 LLM 推断）：
  - 查询侧推断只对 query 做一次，不污染 memory 库
  - 但增加每次 query 成本（一次额外 LLM call）

## 我的推荐（等 potato 拍板）

**混合方案 B'**: 只对 causation 改 prompt："extract **explicit or clearly implied** causation; omit only if truly absent"。保留 participants / temporal 的保守语义（"omit if not mentioned"）。

理由：
- 10 条样本里 4 条有明确因果词（necessitating, lacks... needs, failure modes include, penalizes...）
- 这些是 LLM 应该能抓到的，不是模糊推断
- participants/temporal 强行推断会引入噪声

**估计影响**：如果这个假设在更大样本上成立，causation 覆盖率可以从 13% 提升到 ~50%+，为 ISS-020 dim-aware ranking 提供更多弹药。

## 局限

- **样本太小**（10 条），需要在 LoCoMo 对话数据上再验证一次
- **LoCoMo 是多人对话**，participants 预期会天然比 telegram 记忆高很多（可能 60%+）
- 结论仅适用于当前 engram/telegram 数据分布
