# ISS-001: Recall Intent Classification — Approach Comparison

**Date**: 2026-04-21
**Status**: Investigation Complete, Decision Needed
**Component**: `src/query_classifier.rs`, `src/memory.rs`
**Related**: LoCoMo benchmark analysis (48.3% Cat-1 errors from irrelevant recall)

---

## Problem

engram 存储时用 Haiku LLM 把每条记忆分类（Factual/Episodic/Relational/Procedural/Emotional/Opinion/Causal），但 recall 时**完全忽略这个标签**。7通道融合（FTS/embedding/ACT-R/entity/temporal/hebbian/somatic）没有任何一个通道利用 memory_type 信息。

结果：频繁访问的操作性 episodic 记忆通过 ACT-R 频率偏差和 Hebbian 共激活获得过高权重，系统性地压制定义性 factual 记忆。

**解决思路**：在 recall 时判断查询意图（QueryIntent），然后给候选记忆做 type affinity 乘法调制——如果查的是"X是什么"，就给 factual 记忆 ×2.0、episodic 记忆 ×0.5。

**核心难点**：如何在 recall 热路径上准确判断 QueryIntent？

---

## 方案对比（实测数据）

### 方案 A：Regex Pattern Matching

**现状**：coder 已经实现，代码在 `src/query_classifier.rs`。

**实测准确率：78% (14/18)**

| 查询 | 预期 | 结果 |
|------|------|------|
| what is ACT-R activation | definition | ✅ |
| how to configure the memory system | howto | ✅ |
| what happened yesterday | event | ✅ |
| 我在搞recall的优化 | context | ✅ |
| Tim读了什么书 | definition | ❌ → general |
| hello | general | ✅ |
| potato和engram的关系 | relational | ✅ |
| OpenClaw用了什么技术栈 | definition | ❌ → general |
| 上次部署出了什么问题 | event | ✅ |
| tell me about the recall pipeline | definition | ✅ |
| 怎么重启OpenClaw | howto | ❌ → general |
| I am building a new feature for engram | context | ❌ → general |

**误判分析**：
- 4个错误全是 → General（漏检，不是误检）
- "Tim读了什么书" 没命中是因为"读了什么"不在 definition 的 pattern 里
- "怎么重启" 没命中是因为只有"怎么做/怎么搞/怎么弄"，没有"怎么+动词"
- "I am building" 没命中是因为只有 "i'm building"，没有 "i am building" + 非 working on

**优势**：
- 零延迟（µs 级）
- 零成本
- 漏检时 fallback 到 General（所有 affinity = 1.0，不会变差）

**劣势**：
- 非问句场景（"OpenClaw auth有个bug"）完全无法分类
- 中文 pattern 覆盖率不够
- 需要不断手动补 pattern

---

### 方案 B：Embedding 锚点分类（用现有 nomic-embed-text）

**原理**：预计算每个 intent 的锚点向量，query embedding 和锚点做 cosine similarity，取最高。

**实测准确率：20% (2/10)**

| 查询 | 预期 | 结果 | 备注 |
|------|------|------|------|
| what is ACT-R activation | definition | ❌ → howto | spread=0.128 |
| Tim读了什么书 | definition | ❌ → howto | spread=0.108 |
| potato和engram的关系 | relational | ❌ → howto | spread=0.046 |

**根因**：nomic-embed-text 是**通用语义相似度模型**，所有查询的 embedding 和所有锚点的 similarity 都在 0.45-0.75 之间，差异极小（spread 平均 0.09）。模型学的是"语义相近"，不是"意图分类"。

**用 paraphrase-multilingual-MiniLM-L12-v2 测试**：准确率提升到 50%，但仍然不够。spread 更大但仍有大量误判。

**结论：Embedding 锚点方案在 intent 分类任务上本质不可行。** 通用 embedding 模型的设计目标是语义相似度，不是 intent 判别。所有 intent 的锚点文本本身语义就很相似（都是"问某种问题"），导致区分度太低。

---

### 方案 C：NLI Zero-Shot Classification（cross-encoder/nli-deberta-v3-small, 142M参数）

**原理**：NLI（自然语言推理）模型判断 "premise entails hypothesis"。用 zero-shot pipeline 让模型判断 query 最可能属于哪个 label。

**实测准确率：60% (6/10)**

| 查询 | 预期 | 结果 | 置信度 |
|------|------|------|--------|
| what is ACT-R activation | definition | ✅ | 0.890 |
| how to configure the memory | howto | ❌ → definition | 0.530 |
| Tim读了什么书 | definition | ✅ | 0.751 |
| 我在搞recall的优化 | context | ❌ → event | 0.589 |

**延迟**：~150-190ms/query（CPU，Mac mini M4）
**模型大小**：~280MB (safetensors)

**优势**：
- 比 embedding 锚点好很多
- 能处理自然语言表述变体
- ONNX 导出可加速

**劣势**：
- 中文效果不理想（NLI 训练数据以英文为主）
- 需要引入 Python 依赖或 ort/ONNX Rust crate
- 150ms 延迟不算零
- label 描述需要精心调优

---

### 方案 D：Local LLM（Ollama 小模型）

**实测结果**：

| 模型 | 大小 | 准确率 | 延迟 | 备注 |
|------|------|--------|------|------|
| qwen3:0.6b | 600M | 0% (0/12) | 300ms | thinking mode 输出为空，无法使用 |
| gemma3:1b | 1B | 33% (4/12) | 250ms | 严重偏向 "howto"，几乎所有东西都判成 howto |
| smollm2:135m | 135M | 8% (1/12) | 70ms | 不遵循指令，输出长文本 |

**结论：<1B 的 generative 模型做分类不可靠。** 它们要么不遵循格式指令，要么有严重的 label 偏差。想用 local LLM 做分类至少需要 3B+ 模型（如 qwen3:4b 或 gemma3:4b），但这意味着 500ms+ 延迟和 2.5GB+ 内存。

---

### 方案 E：Haiku API（作为 Level 2 fallback）

**预估**：
- 准确率：>90%（基于 Haiku 的一般分类能力）
- 延迟：200-400ms（网络 roundtrip）
- 成本：~$0.0001/query（~100 tokens）
- 可以和 recall 的 embedding 计算并行，实际延迟 = max(Haiku, embedding) ≈ 200ms

**未实测**（但 Haiku 在存储时的 memory_type 分类已经在用，表现稳定）

---

## 综合对比

| 方案 | 准确率 | 延迟 | 成本 | 中文 | 非问句 | 依赖复杂度 |
|------|--------|------|------|------|--------|------------|
| A. Regex | 78% | µs | $0 | 一般 | ❌ | 零 |
| B. Embedding 锚点 | 20% | 0ms* | $0 | ❌ | ❌ | 零（复用现有） |
| C. NLI (DeBERTa) | 60% | 150ms | $0 | 差 | ✅ | ONNX runtime |
| D. Local LLM (<1B) | 0-33% | 70-300ms | $0 | 差 | ❌ | Ollama |
| E. Haiku API | ~90%+ | 200ms | $0.0001 | ✅ | ✅ | 已有 |

*B 的 0ms 是指锚点比较的开销，embedding 计算本身是复用的

---

## 建议方案

### **推荐：A + E 两级 (Regex + Haiku fallback)**

```
用户消息进来
  → Level 1: Regex (µs)
      → 匹配到明确 intent → 直接用，不调 API
      → 匹配到 General → Level 2

  → Level 2: Haiku API (并行于 recall embedding 计算)
      → 返回 intent
      → 用于 type affinity 乘法
```

**为什么不用 local 方案：**
1. **Embedding 锚点 20% 准确率**——方案从根本上不成立
2. **NLI 模型 60% + 中文差**——不如 Haiku 且需要新依赖
3. **Local LLM <1B 全军覆没**——分类能力不够
4. **Local LLM 3B+ 可能行，但**需要 2.5GB+ 常驻内存 + 500ms 延迟，不如 Haiku

**为什么 Haiku 可接受：**
1. 延迟被 recall embedding 计算掩盖（并行）
2. 成本可忽略（$0.0001/query）
3. 中英文都好
4. 已有 Haiku provider 代码可复用

### 进一步优化 Regex（缩小 Level 2 触发率）

当前 regex 的 4 个漏检可以通过增加 pattern 修复：
- "Tim读了什么书" → 加 `什么书/什么东西/什么人` pattern
- "怎么重启" → 加 `怎么+\w+` (怎么+任意动词) pattern  
- "I am building" → 补全 context pattern
- "OpenClaw用了什么" → 加 `用了什么/用的什么` pattern

修复后 regex 准确率可到 **85-90%**，Level 2 触发率降到 10-15%。

---

## 实现计划

### Phase 1: 优化 Regex（已有代码基础上改）
- 补充中文 pattern（怎么+动词、什么+量词、用了什么）
- 补充英文 pattern（I am + -ing、短事实问句 who/what/where + 名词）
- 预期：78% → 85-90%
- 改动量：~30 行 pattern

### Phase 2: Haiku Level 2 Fallback
- 在 `memory.rs` recall 方法中，当 regex 返回 General 时，并行发 Haiku 请求
- 用 `tokio::join!` 并行 Haiku + embedding 计算
- 需要解决：`memory.rs` 目前是 sync 的，Haiku 调用需要 async
  - 选项 1：用 `reqwest::blocking` 在线程池调用（简单）
  - 选项 2：把 recall 改成 async（大改）
- 改动量：~100 行

### Phase 3: Benchmark 验证
- 重新跑 LoCoMo benchmark，对比 Cat-1/Cat-5 错误率变化
- A/B 测试：有/无 type affinity 的 recall 准确率

---

## 当前代码状态

coder 已实现（在 dirty working tree 中）：
- ✅ `QueryIntent` enum (Definition/HowTo/Event/Relational/Context/General)
- ✅ `TypeAffinity` struct + 映射表
- ✅ `IntentAnchors` (embedding 锚点分类) — **注意：实测不可靠，需要移除或降级**
- ✅ `classify_query_with_embedding()` 两级分类（regex + embedding L2）
- ✅ `memory.rs` type affinity 乘法调制
- ✅ 20+ 测试
- ⚠️ 1 个 pre-existing 测试失败 (`somatic_marker_boosts_emotional_memories`) — 可能受 type affinity 影响

**需要修改**：
- Level 2 从 embedding 锚点换成 Haiku API fallback
- 补充 regex pattern 提高覆盖率
- 验证 somatic 测试失败是否由本次改动引起
