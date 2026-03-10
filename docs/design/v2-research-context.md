# Engram v2 Research Context

> 本文档汇总了 v2 设计的研究背景、认知科学依据、竞品分析和架构决策。
> Coding agent 请先读本文档了解 WHY，再读具体设计文档了解 HOW。

---

## 1. Engram 的定位：智能的第三条路

AI 系统实现 "智能" 有三条路径：

| 路径 | 代表 | 原理 | 优势 | 局限 |
|------|------|------|------|------|
| **统计学习** | GPT/LLM | 从海量文本学概率分布 → 涌现 "智能" | 规模化、通用 | 无持久记忆、无因果推理 |
| **结构复制** | 果蝇连接组 (FlyWire) | 1:1 扫描生物大脑结构 → 直接模拟 | 生物真实 | 静态快照、无法学习新东西 |
| **认知建模** | ACT-R / Engram | 用数学公式抽象大脑的运作规则 | 可工程化、可迭代 | 是简化模型，非完整大脑 |

**Engram 走第三条路**：不复制大脑结构（太贵太慢），不做纯统计学习（没有记忆动力学），而是把认知科学家 40 年研究出的数学模型直接应用到 AI agent memory。

### 果蝇连接组的启示

FlyWire 项目（Dorkenwald et al., Nature 2024）：
- 139,255 个神经元 + 54.5M 突触连接
- 电子显微镜逐层扫描 → **100% 静态快照**（果蝇已死）
- 无法捕获：突触权重（化学过程）、可塑性规则、实时动态
- 存储：~6GB（图结构），~21PB（原始扫描）
- 计算：单张 GPU 可实时模拟（point neuron model）

**关键结论**：结构复制能产生基本反应，但无法学习新东西。真正的智能需要 **动态** — 连接怎么变强/变弱/形成/消失。这正是 Engram 的 Hebbian learning + forgetting + consolidation 在做的事。

---

## 2. 现有架构审计

### 已实现的认知机制（v1 完整）

| 机制 | 认知科学来源 | 实现状态 | 大脑对应 |
|------|-------------|---------|---------|
| **ACT-R Base-level Activation** | Anderson (1993) | ✅ `activation.py` | 海马体检索 |
| **Spreading Activation** | Anderson (1993) | ⚠️ 部分（需要手动传 context_keywords） | 语义网络扩散 |
| **Hebbian Learning** | Hebb (1949) | ✅ `hebbian.py` | 突触可塑性 (LTP) |
| **Ebbinghaus Forgetting** | Ebbinghaus (1885) | ✅ `forgetting.py` | 记忆衰减 |
| **Contradiction Detection (RIF)** | Anderson et al. (1994) | ✅ `-3.0` penalty | 检索诱发遗忘 |
| **Importance Weighting** | — | ✅ `entry.importance` | 杏仁核情绪标记（简化版）|
| **Memory Consolidation** | Murre & Chessa (2011) | ✅ `consolidation.py`（手动调用） | 睡眠记忆整理 |
| **Memory Types & Layers** | Squire (1992) | ✅ EPISODIC/SEMANTIC/PROCEDURAL, L1-L4 | 多重记忆系统 |
| **Interleaved Replay** | McClelland et al. (1995) | ✅ 随机采样 archive boost | 海马体重播 |
| **Adaptive Parameter Tuning** | — | ✅ `AdaptiveTuner` | 无直接对应（工程特性） |
| **Session Working Memory** | Baddeley (1992), Miller (1956) | ✅ 设计完成 | 前额叶工作记忆 |

### 已知 Gap（v1 → v2 要补的）

| Gap | 影响 | 对应大脑机制 | 优先级 |
|-----|------|-------------|--------|
| **Episodic→Semantic 提炼** | 重复 episode 堆积，检索噪音增加 | 海马体→新皮层记忆转换 | 🔴 P0 |
| **Pattern Classification** | 无法区分"偏好"vs"近期热点" | 前额叶元认知 | 🔴 P0 |
| **Priming** | 相关记忆不会被预激活 | 语义启动效应 | 🔴 P0 |
| **Temporal Prediction** | 不能基于时间模式推送记忆 | 时间编码 | 🟡 P1 |
| **Schema Formation** | 无法从重复经验提取抽象模板 | 前额叶 schema | 🟡 P1 |
| **Emotion/Anomaly Weighting** | 异常/突发信息无特殊处理 | 杏仁核 | 🟡 P1 |
| **Lateral Inhibition** | Top-K 可能有冗余结果 | 侧抑制 | 🟢 P2 |
| **Metamemory** | Agent 不知道自己"知不知道" | 元记忆 | 🟢 P2 |

---

## 3. 核心设计决策

### 3.1 ACT-R 不需要修改

`B_i = ln(Σ t_k^(-d))` 回答的是 **"这条记忆现在有多容易被想起来？"** — 这是检索层的问题，答案就应该是 frequency × recency。

**"activation 高 = 偏好"是错误的推断。** Activation 高只意味着 "当前可及性高"。区分偏好 vs 近期热点是 **提炼层（distillation）** 的责任，不是检索层的。

分层设计：
```
检索层（ACT-R）  →  "现在该想起哪条记忆？"     → 不改
提炼层（Pattern） →  "这个使用模式意味着什么？"  → 新增
预测层（Schema）  →  "接下来可能需要什么记忆？"  → 新增
```

类比人脑：
- 海马体 = ACT-R（快速检索，不做判断）
- 前额叶 = Pattern Classification + Schema（反思、解释、预测）

### 3.2 不依赖 Embedding 的 Clustering

Episodic→Semantic distillation 需要找到 "同一主题的 episodic 记忆簇"。

**不需要额外的 embedding 或 clustering 算法。** Hebbian links 已经告诉我们哪些记忆经常被一起 recall — 这就是 "同一主题" 的天然信号。用 connected components on Hebbian graph 即可。

### 3.3 Pattern Classification 四种类型

| 类型 | 判断依据 | 衰减率 | 语义记忆格式 |
|------|---------|--------|-------------|
| **PREFERENCE** | 30+ 天跨度 + 多 context + 分散访问 | 0.3（慢） | "User consistently prefers X" |
| **CURRENT_FOCUS** | >70% 集中在最近 14 天 | 0.7（快） | "User is currently working with X" |
| **CONTEXTUAL** | context 多样性 ≤ 1 | 0.5 | "User uses X specifically for Y" |
| **OBSERVATION** | 以上都不满足 | 0.6 | "User has discussed X in N conversations" |

**关键**：CURRENT_FOCUS 的语义记忆如果后续 2 周没有新 episodic 加入，应自动降级。

### 3.4 Predictive Memory 三阶段

| Phase | 机制 | 复杂度 | 依赖 |
|-------|------|--------|------|
| **Priming** | Hebbian 邻居自动预激活 | 低 | 现有 Hebbian links |
| **Temporal** | 时间模式统计 | 中 | access_log 时间分析 |
| **Schema** | 序列模式挖掘 | 高 | session log + pattern mining |

---

## 4. 竞品分析

### 竞品都缺什么

| 特性 | Mem0 (49K⭐) | Letta (21K⭐) | Cognee (13K⭐) | Memvid (11.7K⭐) | Zep (4K⭐) | **Engram v2** |
|------|-------------|-------------|---------------|-----------------|-----------|--------------|
| 检索 | Embedding | Embedding+LLM | Graph+Embedding | Embedding | Embedding | **ACT-R+Hebbian** |
| 遗忘 | 手动 | 无 | 无 | 无 | TTL | **Ebbinghaus 衰减** |
| 关联发现 | 无 | 无 | Graph (LLM提取) | 无 | 无 | **Hebbian (自动涌现)** |
| Episodic→Semantic | 无 | 无 | 无 | 无 | 无 | **✅ 自动提炼** |
| 预测性推送 | 无 | 无 | 无 | 无 | 无 | **✅ Priming+Schema** |
| 偏好 vs 热点 | 无 | 无 | 无 | 无 | 无 | **✅ Pattern Classification** |
| LLM 依赖 | 需要 | 需要 | 需要 | 可选 | 需要 | **可选 ($0 离线)** |
| 认知科学基础 | 无 | 无 | 无 | 无 | 无 | **ACT-R, Hebb, Ebbinghaus** |

**没有任何竞品在做 episodic→semantic distillation 或 predictive memory。** 这是 Engram v2 的独有特性。

### Benchmark 现状

现有 benchmark 都不测 Engram 关心的特性：

| Benchmark | 测什么 | 测 Engram 特性吗？ |
|-----------|--------|-------------------|
| MTEB | 通用 embedding 质量 | ❌ |
| BEIR | 信息检索 | ❌ |
| LoCoMo | 长对话记忆 | ⚠️ 只测 "记不记得" |
| MemoryArena (2026) | 多 session agent 记忆 | ⚠️ 最接近，但不测排序质量 |

**Engram 需要的 benchmark 维度**（自建 CogMemBench）：
1. **Temporal Retrieval** — 结果是否随 access pattern 变化？
2. **Forgetting Precision** — 旧 noise 是否自然退出 top-K？
3. **Association Discovery** — 未显式声明的关联能否被发现？
4. **Scale Degradation** — 精度是否随 memory 量下降？

---

## 5. 生产数据参考

当前 v1 在 OpenClaw 上 30+ 天连续运行的数据：

| 指标 | 数值 |
|------|------|
| 总记忆 | 3,846 条 |
| 总检索 | 230,103 次 |
| Hebbian links | 12,510 个（自动涌现） |
| 平均检索时间 | ~90ms |
| 存储 | 48MB (SQLite) |
| 基础设施成本 | $0 |
| 运行时长 | 30+ 天 |

---

## 6. v2 实现指南

### 要读的文件（按顺序）

1. **本文档** — 研究背景和设计决策 WHY
2. `docs/COGNITIVE_MECHANISMS_AUDIT.md` — v1 现有 pipeline 完整审计
3. `docs/design/bot-architecture-thesis.md` — 核心设计哲学
4. `docs/design/episodic-semantic-distillation.md` — v2 功能：Episodic→Semantic 提炼
5. `docs/design/predictive-memory-schema.md` — v2 功能：预测性记忆 + Schema

### 实现优先级

```
Phase 1 (1 天): Pattern Classification
    └── classify_pattern() — 区分 PREFERENCE/CURRENT_FOCUS/CONTEXTUAL/OBSERVATION
    └── 加到 consolidation pipeline

Phase 2 (2-3 天): Episodic→Semantic Distillation (offline 版)
    └── Hebbian graph clustering → gist extraction → semantic memory creation
    └── 先实现 Option B (TF-IDF, 不依赖 LLM)
    └── 数据模型变更 (source_episodes, distilled_from_count)

Phase 3 (1-2 天): Priming
    └── recall() 时自动拉入 Hebbian 邻居
    └── prediction_boost 临时加分（不改变原始 activation）

Phase 4 (2-3 天): Temporal Prediction
    └── access_log 时间分析
    └── predict() API

Phase 5 (1 周): Schema Formation
    └── session log 基础设施
    └── 序列模式挖掘 (简化 PrefixSpan)
    └── schema 数据模型 + 匹配 + 预测
```

### 关键约束

1. **不改 ACT-R 公式** — `B_i = ln(Σ t_k^(-d))` 保持不变
2. **不强制依赖 LLM** — 所有功能都要有 offline fallback
3. **不强制依赖 embedding** — 用 Hebbian graph + FTS5 做 clustering
4. **向后兼容** — 现有 `Memory` API 不 break
5. **渐进式** — 每个 Phase 独立可用，不需要全做完才有价值
6. **测试覆盖** — 每个新功能都要有 pytest，参考设计文档中的测试计划

### 代码风格参考

看现有的：
- `engram/activation.py` — ACT-R 实现
- `engram/hebbian.py` — Hebbian learning
- `engram/forgetting.py` — Ebbinghaus forgetting
- `engram/consolidation.py` — Memory consolidation

新代码应保持一致风格：纯 Python、类型注解、docstring 引用认知科学来源。

---

*Document created: 2026-03-09*
*Author: Impact & Visibility Bot (research + design), implementation by coding agent*
