# Engram 记忆系统调研 — 与市面记忆系统对比 & 改进路线图

> 调研日期：2026-03-31 ~ 2026-04-02
> 状态：进行中，需后续详细讨论
> 相关文档：`INVESTIGATION-2026-03-31.md`（生产环境问题调查）、`ENGRAM-V2-DESIGN.md`（情感总线架构）

---

## 1. 市面记忆系统全景对比

### 1.1 Hindsight (vectorize-io)

**定位**：SaaS 级 agent memory（PostgreSQL + Docker + server/client）

**核心架构**：
- 4 层记忆：World Facts → Experiences → Observations → Mental Models
- **TEMPR 4 路检索**：Semantic + BM25 + Graph + Temporal，用 RRF 融合排序
- **Observation Consolidation**：自动从多条 facts 合成高阶 insight（最核心能力）
- `reflect()`：LLM 在记忆之上做推理
- Entity Graph：自动提取实体和关系
- LongMemEval benchmark 91.4%

**可借鉴**：
1. **Observation Consolidation** — engram 最大缺口。Hindsight 的 consolidate 不是"加强"记忆，而是"合成新知识"
2. **Entity Graph** — 结构化的实体关系网络，比 Hebbian link 更有意义
3. **多路检索融合** — 目前 engram 只有 FTS5 + ACT-R，缺 embedding 向量搜索和 entity graph

**不适合我们的**：
- 重量级（PostgreSQL + Docker + 服务端）
- 面向 SaaS 客户，不是嵌入式 crate
- 不开源核心 consolidation 算法

### 1.2 Mem0 (mem0ai)

**定位**：轻量 agent memory layer，Python SDK

**核心架构**：
- **双阶段 Extract + Reconcile**（最核心设计）
  - Stage 1 (Extract): LLM 从对话中提取结构化 facts
  - Stage 2 (Reconcile): 新 fact 与已有记忆对比，决定 ADD / UPDATE / DELETE / NOOP
- Graph Memory（可选）：Neo4j entity 关系图
- 支持多 provider：OpenAI, Qdrant, ChromaDB 等
- Memory 有 version history

**可借鉴**：
1. **Reconcile 阶段** — 这是 engram 缺失的关键环节。目前 engram 只 extract 不 reconcile，导致重复记忆堆积
2. **ADD/UPDATE/DELETE/NOOP 操作** — 不是所有新 fact 都应该创建新记忆，有些应该更新已有记忆
3. **Version history** — 追踪记忆的演变（"potato 喜欢 Python" → "potato 更喜欢 Rust"）

**不适合我们的**：
- Python（我们是 Rust crate）
- 需要外部 vector DB（Qdrant/ChromaDB）
- Reconcile 的 LLM 调用成本高

### 1.3 Zep

**定位**：长期记忆 + 时间推理

**核心架构**：
- **Temporal Graph**：记忆自动按时间链接，支持时间范围查询
- **Fact Extraction**：从对话自动提取 facts（类似 Mem0）
- **User/Session 分层**：用户级 facts + 会话级 context
- **Temporal Reasoning**：可以回答 "上周三讨论了什么？"

**可借鉴**：
1. **时间推理** — engram 有 created_at 但没有 temporal query 能力
2. **User/Session 分层** — 类似 namespace 但更细粒度

**不适合我们的**：
- 偏 SaaS 产品
- 过于关注 user-facing chatbot 场景

### 1.4 LangMem (LangChain)

**定位**：LangChain 生态的 memory 组件

**核心架构**：
- **Mission-Steered Extraction** — extraction prompt 包含 mission statement，指导 LLM 只提取与 agent 使命相关的信息
- Thread-level summarization
- Shared memory across agents

**可借鉴**：
1. **Mission-Steered** — 用 SOUL.md 的驱动引导 extraction，不是无脑提取一切

---

## 2. Engram 现状与差距

### 2.1 Engram 优势（要保留的）

| 能力 | 说明 |
|------|------|
| **嵌入式 Rust crate** | 单 SQLite，零外部依赖，90ms 查询 |
| **ACT-R 认知模型** | 真正的认知科学基础，频率+时近+重要度排序 |
| **Hebbian 学习** | 共现记忆自动关联（STDP 权重） |
| **7 种记忆类型** | factual, episodic, procedural, emotional, relational, opinion, causal |
| **情感总线（v2）** | 情绪追踪 + 趋势分析 + SOUL 驱动对齐 |
| **本地隐私** | 数据不离开本地，不需要云服务 |

### 2.2 核心差距

| 差距 | 严重度 | 参考系统 |
|------|--------|---------|
| **无 Reconcile 阶段** | 🔴 Critical | Mem0 |
| **无 Observation Consolidation** | 🔴 Critical | Hindsight |
| **无 Entity Graph** | 🟡 High | Hindsight, Mem0 |
| **无多路检索融合** | 🟡 High | Hindsight TEMPR |
| **无时间推理** | 🟢 Medium | Zep |
| **无 Mission-Steered Extraction** | 🟡 High | LangMem |
| **Extraction 质量差** | 🔴 Critical | 所有 |

### 2.3 生产环境实际问题

详见 `INVESTIGATION-2026-03-31.md`，核心：
- 1,850 条记忆中 ~13% 是垃圾（heartbeat 指令、状态报告重复）
- Haiku extractor 把系统指令当知识存储
- 没有去重 → 同一条信息存十几次
- Recall 被垃圾稀释，有效信息被挤掉

---

## 3. 改进路线图：Memory Lifecycle 架构

**核心思路**：Root fix 不是简单加 filter，而是建立完整的 Memory Lifecycle。

每一层独立实施，但设计面向最终形态，没有技术债。

### Layer 1: Gate（入口过滤）

**目标**：阻止垃圾进入 lifecycle

**实现**：
- RustClaw 侧：标记会话类型（heartbeat / direct / group），heartbeat 会话直接跳过 EngramStoreHook
- Engram 侧：`store()` 接受 `session_type` 参数，`heartbeat` / `system` 类型直接返回
- 跳过 NO_REPLY、HEARTBEAT_OK、长度 < 20 字符的内容

**工作量**：~2h  
**效果**：立即阻止新垃圾写入

### Layer 2: Mission-Steered Extraction

**目标**：只提取与 agent 使命相关的高质量 facts

**实现**：
- Extraction prompt 注入 SOUL.md 的核心驱动（"帮 potato 实现财务自由"、"技术深度"、"好奇心"）
- 添加 negative examples（heartbeat 指令、状态报告、系统日志 → 不提取）
- Few-shot examples（好的 extraction 长什么样）
- Importance 校准：auto-extracted 上限 0.7，procedural 默认 0.5

**工作量**：~4h  
**效果**：extraction 精度大幅提升

### Layer 3: Embedding-based Reconciler

**目标**：新 fact vs 已有记忆的智能去重

**实现**：
- 新 fact 提取后，用 embedding 搜索 top-K 相似记忆
- 如果 cosine similarity > 0.85 → 不创建新记忆，只更新 access_log（NOOP/UPDATE）
- 如果 0.6 < similarity < 0.85 → 可能是演变，标记待 reconcile
- 如果 < 0.6 → 新信息，正常 ADD

**工作量**：~6h（需要 embedding pipeline）  
**效果**：消除重复记忆

### Layer 4: LLM Reconciler（可选）

**目标**：精确处理记忆演变（UPDATE vs ADD）

**实现**：
- 对 Layer 3 标记的"可能演变"记忆，调 LLM 判断：
  - `ADD` — 新信息，两条都保留
  - `UPDATE` — 更新已有记忆内容
  - `DELETE` — 已有记忆过时，删除旧的
  - `NOOP` — 完全重复，跳过
- 参考 Mem0 的双阶段架构

**工作量**：~8h  
**效果**：记忆自动演进，不积累过时版本

### Layer 5: Observation Consolidation

**目标**：从多条 facts 合成高阶 knowledge

**实现**：
- 定期扫描同 topic 的记忆集群（用 embedding 聚类或 Hebbian link）
- 调 LLM 合成 observation："从这 5 条记忆中，你能总结出什么高阶规律？"
- 新 observation 存为独立记忆，importance 高于原始 facts
- 原始 facts 降低 importance 但不删除（保留证据链）

**工作量**：~12h  
**效果**：从"记事本"进化为"知识库"

### Layer 6: Entity Graph

**目标**：结构化的实体关系网络

**实现**：
- 表已存在（entities / entity_relations / memory_entities），但零业务逻辑
- Extraction 时同时提取 entities 和 relationships
- Recall 时增加一路 graph-based retrieval
- 与 TEMPR 融合排序

**工作量**：~16h  
**效果**：支持关系查询（"potato 用过哪些工具？"）

### Layer 7: Multi-Retrieval Fusion

**目标**：TEMPR 级检索质量

**实现**：
- 4 路检索：FTS5 (keyword) + Embedding (semantic) + Entity Graph + ACT-R (temporal/importance)
- RRF (Reciprocal Rank Fusion) 融合排序
- 动态权重：根据 query 类型自动调整（关键词型 query 偏重 FTS5，语义型偏重 embedding）

**工作量**：~10h  
**效果**：recall 准确率接近 Hindsight 水平

---

## 4. 优先级总结

| 优先级 | Layer | 独立实施 | 累积效果 |
|--------|-------|---------|---------|
| **P0** | Layer 1: Gate | ✅ | 阻止垃圾 |
| **P0** | Layer 2: Mission-Steered Extraction | ✅ | 提高提取质量 |
| **P1** | Layer 3: Embedding Reconciler | ✅ | 消除重复 |
| **P2** | Layer 4: LLM Reconciler | 需 Layer 3 | 记忆演进 |
| **P2** | Layer 5: Observation Consolidation | ✅ | 知识合成 |
| **P3** | Layer 6: Entity Graph | ✅ | 关系查询 |
| **P3** | Layer 7: Multi-Retrieval Fusion | 需 Layer 3+6 | TEMPR 级检索 |

**关键约束**：
- Layer 1-2 纯逻辑改动，不需要新依赖
- Layer 3+ 需要 embedding pipeline（Ollama 本地 or API）
- Layer 4-5 需要 LLM 调用（成本/延迟考量）
- 每层都是独立的 PR，可以单独发 crate 版本

---

## 5. Engram 竞争定位

**Engram 不应该追 Hindsight/Mem0 的 SaaS 方向。**

差异化竞争力：
1. **嵌入式 Rust crate** — 唯一的 Rust 原生 agent memory（零 Python、零 Docker、零外部 DB）
2. **认知科学基础** — ACT-R + Hebbian 不是噱头，是真正影响 recall 质量的数学模型
3. **情感总线** — 没有竞品有这个（情绪追踪 → 个性演化 → 行为调整）
4. **多 agent 共享** — namespace + ACL，一个 SQLite 服务整个 agent swarm

**目标**：不是做最大的记忆系统，而是做最好的嵌入式 agent memory。

---

## 6. 待讨论问题

1. **Auto-store vs Agent 主动存**：是否应该关闭 auto-store，完全依赖 agent 主动调用 `engram_store`？
2. **Embedding provider 选择**：Ollama 本地 vs API？延迟/质量/成本 trade-off
3. **LLM Reconciler 的调用时机**：同步（store 时立即 reconcile）vs 异步（batch reconcile）
4. **Observation Consolidation 的触发条件**：定时？记忆数量阈值？Hebbian link 强度？
5. **Entity Graph 的 schema**：复用现有空表还是重新设计？
6. **crate API 设计**：Layer 3-7 是否需要新的 trait / config 接口？

---

## 7. 状态机 + 神经元激活热图（2026-04-10 potato 提出）

### 7.1 问题

Agent 重启 = 完全失忆。当前的解决方案（MEMORY.md + daily log + engram recall）是"翻旧笔记"模式——agent 被动地去搜索过去，而不是主动知道"我刚才在做什么"。

这跟人脑不同。人睡一觉醒来，不需要翻日记才知道昨天在做什么——大脑有持续的激活状态，相关区域仍然"亮着"。

### 7.2 两个层面

#### Layer A: Working Context State Machine（显式状态）

类比 GID ritual 的状态机，给 engram 加一个 **working context** 层：

```
状态转移：idle → active(task) → blocked(reason) → idle
```

具体存储：
- `current_task`: 当前正在做什么（"ISS-007 修复"、"讨论 engram 架构"）
- `conversation_with`: 跟谁在交互
- `topic`: 当前话题
- `status`: working / waiting_for_user / blocked / idle
- `last_checkpoint`: 最后一个有意义的操作（"已修复 3/4 子任务，等用户确认第 4 个"）

**特点**：离散的、显式的状态转移。Session 结束时自动 save，启动时自动 load。Agent 一醒来就知道"上次在做什么、做到哪了"。

不是记忆，是**工作台**。类似你关上笔记本电脑再打开，所有窗口还在那里。

#### Layer B: Activation Heatmap（隐式热度）

利用 engram 已有的 ACT-R activation 模型，但把它从"被动排序工具"变成"主动感知信号"。

核心想法：Session 启动时，扫描所有记忆的当前 activation 值，生成一个 **Top-K 概念热图**：

```
🔴 Hot (activation > 0.8):  engram, confidence, ISS-007, memory-fix
🟡 Warm (0.5-0.8):          consolidation, neuroscience, Hindsight
🟢 Cool (0.3-0.5):          xinfluencer, AgentVerse, trading
```

**这就是"亮着的神经元区域"**。Agent 不需要被告知"我们最近在搞 engram 修复"——它自己能感知到，因为那片区域的 activation 最高。

实现方式：
1. 对所有记忆算 ACT-R activation（已有公式：频率 × 时近 × 重要度）
2. 从记忆内容提取关键概念（tags / 关键词）
3. 聚合同概念记忆的 activation，得到概念级热度
4. Top-K 注入 system prompt，作为"当前认知焦点"

**跟 Hindsight 的 Observation 不同**：Observation 是合成新知识（"potato 是 Python 死忠"），Activation Heatmap 是感知当前注意力焦点（"最近在搞 engram"）。两者互补，不冲突。

### 7.3 与现有架构的关系

| 机制 | 类型 | 存储 | 用途 |
|------|------|------|------|
| ACT-R Activation | 连续值 | 内存计算 | recall 排序（已有） |
| Hebbian Links | 连续权重 | SQLite | 关联记忆（已有） |
| **Working Context** | 离散状态 | SQLite 新表 | session 持续性（新增） |
| **Activation Heatmap** | 连续聚合 | 启动时计算 | 认知焦点感知（新增） |

### 7.4 跨 Agent 共享场景

Working Context + Activation Heatmap 天然支持多 agent 共享：
- Agent A 在做 engram 修复 → save working context
- Agent B 接手（或共享 namespace）→ load working context → 立刻知道 A 在做什么、做到哪了
- Activation Heatmap 通过共享 SQLite 自动同步——A 大量 access engram 相关记忆 → B 启动时看到 engram 区域是亮的

### 7.5 优先级建议

- **Working Context**: P1，实现简单（一个 key-value 表 + session hook），效果立竿见影
- **Activation Heatmap**: P2，需要关键词提取 + 聚合逻辑，但不需要 LLM 调用
- 两个都不依赖 Layer 3-7 的 embedding / LLM pipeline，可以独立实施

---

*文档地址：`/Users/potato/clawd/projects/engram-ai-rust/MEMORY-SYSTEM-RESEARCH.md`*
*后续在此文档上继续迭代。*
