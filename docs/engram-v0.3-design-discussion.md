# Engram v0.3 设计讨论记录

> **Date**: 2026-04-23
> **Participants**: potato + RustClaw
> **Status**: Design discussion — not yet a formal DESIGN.md
> **Next step**: Study mem0 / A-MEM / Letta / LightRAG in detail, then write DESIGN-v0.3.md

---

## 0. 定位（Mission Statement）

**Engram = 一个完整的 agent memory system，以神经科学为根基。**

- 它既能像 Graphiti 那样回答结构化事实问题，也能像大脑那样做情感语境检索、衰减、联想。
- 它不是 "episodic-only"，它是"**认知上完整的**"。
- 必须是**真正的、独立的、能直接使用的**记忆系统。

**核心定位调整（相对之前）：**
- 放弃"engram 只做 episodic memory，factual 交给别的系统"这个自我安慰的借口
- 承认硬能力（entity / relation / precision）欠缺就是欠缺
- 神经科学特色（ACT-R / somatic / consolidation）是**差异化价值**，要保住
- 硬能力 + 认知层，两者都要

---

## 1. 讨论的起点

### 1.1 最初问题
Extract 时跨 turn 信息如何处理？例如 locomo D3:16 问题——需要把多个 turn 的信息组合才能回答。

### 1.2 初步观察
- 业界已验证有效：**Graphiti**（25.3k stars）就是做这个的
- LangChain 更早做过 ConversationKGMemory
- 最小闭环：extract 前 recall 同话题已有 facts，拼进 prompt

### 1.3 关键转折
potato 问："extractor 抽出来的 metadata，算不算 Graphiti 结构化类似的东西？"

**答**：信息容量等价，但**差一层关键能力**——engram 有 metadata 描述但没有 entity identity。

```
Engram: participants: "Melanie, Marcus"     ← 字符串，两次提到互不认识
Graphiti: Edge(src=n1(Melanie), tgt=n2(Marcus), relation=MARRIED_TO)  ← 指针，共享身份
```

Engram 现在像"**一堆标注过 metadata 的便签**"，不是图。

### 1.4 "越做越像 GID" 的观察
potato 的直觉观察——不是担忧，是正向信号：
- GID 和 engram 在解决同类问题（把非结构化输入抽成节点 + 关系）
- 底层机制相似：extract → graph → query → consolidate
- 可以借鉴 GID 的成熟经验（但不等于合并）

---

## 2. Engram 作为记忆系统的显著缺陷

### 2.1 A 类：认知记忆本身做得不够好
- **缺陷 3**：Extraction 是一次性的、盲的，抽不到就永远丢了
- **缺陷 4**：Recall 被动，consolidation 机制存在但很弱
- **缺陷 5**：没有 working memory 层

### 2.2 B 类：知识系统的硬能力欠缺
- **缺陷 1**：没有 entity identity
- **缺陷 2**：没有跨 memory 的 structured relation
- **缺陷 6**：Factual accuracy 不保证
- **缺陷 7**：没有 schema / ontology
- **缺陷 8**：Retrieval 精度弱

**结论**：A 类和 B 类都要补。不是"定位问题"，是**全方位提升**。

---

## 3. Graphiti 深度分析

### 3.1 Graphiti Pipeline（每 episode 5-10 次 LLM calls）

**固定 calls：**
1. Extract entities
2. Extract edges/facts

**条件 calls：**
3. Entity dedup via LLM（per ambiguous entity）
4. Edge resolution（per new edge: duplicate / invalidate / new）
5. Timestamp extraction
6. Attribute extraction for entities
7. Edge contradiction resolution

**为什么这么多**：Graphiti 选择"每个决策都让 LLM 判断"——精确性优先于效率。

### 3.2 Graphiti 做对的（要学）

1. 显式 entity + edge（UUID、identity）
2. 双向时间戳（valid_at + invalid_at）
3. Invalidation 而非 deletion
4. Episode 作为 provenance
5. Hybrid search（向量 + BM25 + graph traversal）
6. Edge type signatures（(Person, Person) → [MARRIED_TO, ...]）
7. Community detection
8. 增量更新

### 3.3 Graphiti 的真实缺陷（engram 超越点）

| # | 缺陷 | Engram 超越点 |
|---|---|---|
| 1 | LLM 成本爆炸（5-10 calls/episode） | 多信号融合，简单 case cheap path，模糊 case 才 LLM |
| 2 | 没有 importance-based 遗忘（只有 invalidate） | ACT-R activation + decay：区分"还对吗"和"还重要吗" |
| 3 | 没有情感/语境维度 | valence / arousal / domain / importance 已有 |
| 4 | Retrieval 纯 query-driven，无 affect-driven | interoceptive hub + somatic → affect-driven recall |
| 5 | 没有 consolidation / abstraction 层级 | Murre & Chessa consolidation + Knowledge Compiler |
| 6 | Entity resolution 二元决策（太贪心或太保守） | 允许"模糊 entity"，带置信度的概率身份 |
| 7 | Schema 僵化（预定义 edge type map） | Emergent schema（从数据涌现 relation types） |
| 8 | 依赖 Neo4j，不能嵌入 | SQLite 嵌入式，部署简单 |
| 9 | 没有 metacognition | interoceptive 的 confidence/uncertainty 信号 |
| 10 | 单用户思维，无跨 session/user generalization | 跨 session consolidation，federated memory 方向 |

---

## 4. 关键概念澄清

### 4.1 Engram 的遗忘 ≠ Graphiti 的 invalidate

两者正交、可共存：

**Graphiti invalidate：**
- 二元（valid / invalid）
- 事件驱动（新 fact 矛盾时）
- 回答：**"这事还对吗？"**

**Engram decay/archive：**
- 连续（activation ∈ [0, 1]）
- 自动（ACT-R 本性）
- 回答：**"这事还重要吗？"**

Engram 可以两个都做：activation decay（重要性）+ superseded_by（正确性）。

### 4.2 Consolidation vs Knowledge Compiler

**两个都叫 "consolidation" 但在不同层面：**

```
L4: Knowledge Topics（主题页）          ← Knowledge Compiler（横向归纳）
       ↑ 聚合、归纳、冲突消解
L3: Core Memory（r2, 长期皮层痕迹）     ← Consolidation（垂直强度演化）
       ↑ 海马→皮层转移 (Murre & Chessa 2011)
L2: Working Memory（r1, 短期海马痕迹）
```

- **Consolidation**（`models/consolidation.rs`）：单 memory 强度随时间演化，微分方程 `dr1/dt = -μ1·r1`, `dr2/dt = α·r1 - μ2·r2`
- **Knowledge Compiler**（`compiler/`）：多条 memory 聚合成 topic 页面，discovery → compilation → lifecycle

Graphiti 两个都没有 → engram 两方面都领先。

**但是**：Knowledge Compiler 目前"已实现但待打磨"，默认没开启、用得少、稳定性未验证。重构时要重新审视。

---

## 5. 质量 vs 成本的权衡（关键设计决策）

### 5.1 错误方向：为省钱牺牲质量
> ~~"可以把 LLM calls 压到 1-2 次"~~ ← 收回，这是贪便宜

### 5.2 正确方向：多信号融合 + LLM 兜底 + offline audit

**Cheap path 会出错的真实场景：**
1. 同名不同人（false merge）
2. 同人不同名（false split）
3. 相关但不同实体（语义混淆）
4. Edge 矛盾识别不了
5. 时间歧义

这些错误是**复利累积**的——几个月后图会劣化。所以必须认真处理。

### 5.3 分层策略

**Cheap path 可靠的场景：**
- A：同 episode 内 exact match（~100%）
- B：embedding > 0.92 + name normalize 一致 + 无属性冲突（~95%）
- C：embedding < 0.5（几乎 0 误差）
- D：空图或完全新实体

**必须 LLM 的场景：**
- E：中等相似度（0.6 - 0.9）
- F：可能矛盾的 edge
- G：时间推断
- H：属性合并

### 5.4 Engram 独有的信号（相对 Graphiti）

1. **ACT-R activation** — 老 entity 最近是否被 recall 过
2. **Hebbian co-occurrence** — 候选 entity 和哪些老 entity 共同出现过
3. **Temporal proximity** — 上一个 episode 刚提到的，极大概率同一个
4. **Affect continuity** — valence 匹配
5. **Domain consistency** — domain 一致性
6. **Somatic marker match** — 情感指纹匹配

**融合公式（草案）：**
```
confidence = weighted_fusion(
    embedding_cos,
    name_exact_match,
    actr_activation,
    hebbian_strength,
    temporal_proximity,
    affect_match,
    domain_match,
    somatic_match
)

if confidence > 0.85  → auto-merge (cheap)
if confidence < 0.30  → auto-new (cheap)
else                  → LLM resolve (expensive, necessary)
```

### 5.5 Offline Audit（Graphiti 没有）

Consolidation cycle 时跑后台质量检查：
- 小模型抽样扫图："这些合并对吗？"
- 发现错误 → 自动 split
- 发现遗漏 → 自动 merge
- **错误自愈**

组合：**online cheap + LLM 兜底 + offline audit** → 综合质量可能 ≥ Graphiti，成本 < Graphiti。

### 5.6 成本估算（修正后）

- 简单 episode：2-3 次 LLM
- 中等 episode：3-5 次
- 复杂 episode：5-8 次
- **平均省 40-60% LLM 成本，质量持平**

---

## 6. 其他值得学习的记忆系统

### 优先级排序

**1. mem0** (GitHub 35k+ stars)
- 学：API 简洁性
- 缺：无图、无时间、无精细 retrieval

**2. A-MEM** (Princeton, 2024)
- 学：Zettelkasten-style agentic memory，note 的自主演化（新 memory 触发老 note 自我更新）
- 缺：单机、无时间戳、无 entity identity

**3. Letta / MemGPT** (UC Berkeley, 2023)
- 学：让 agent 自己管理分层记忆（function calls 决定移动）
- 缺：LLM 驱动成本高、retrieval 精度一般

**4. LightRAG** (HKU, 2024)
- 学：hybrid retrieval（dual-level：low-level entity / high-level topic）
- 缺：为 RAG 而生，不是 agent memory

**5. GenerativeAgents** (Stanford, 2023)
- 学：reflection 机制（LLM 定期从 memory 提炼 insight）
- Knowledge Compiler 灵感部分来自这里

**已深入讨论：Graphiti**

---

## 7. Roadmap（现实估算）

### 7.1 诚实评估
"几天做完" 不现实。实际需要：

**(A) 补齐 Graphiti 硬能力**
- Entity node 层
- Typed edges
- Entity resolution pipeline（三层 dedup）
- Edge resolution pipeline（duplicate / invalidate / new）
- Temporal validity
- Provenance
- Hybrid search
- Schema 机制

**(B) 超越 Graphiti 的新能力**
- 多信号融合的 cheap path
- Affect-driven retrieval
- Fuzzy entity（概率 identity）
- Emergent schema
- Offline audit

**(C) 整合现有 engram 能力**
- ACT-R / consolidation / Knowledge Compiler 和新图模型耦合
- Somatic / Interoceptive 和新 recall 路径整合
- Extractor 重构（看已有 graph context）

**(D) 工程保障**
- Schema migration
- Benchmark 重跑（locomo / LongMemEval）
- API 兼容（engramai 是已发布 crate）
- 测试（280+ tests 大部分受影响）

### 7.2 分阶段

- **阶段 0（1-2 天）**：设计对齐 → DESIGN-v0.3.md
- **阶段 1（2-3 周）**：核心图层（entity + edge + resolution），能跑、有测试
- **阶段 2（4-6 周）**：整合（新图 ↔ ACT-R / interoceptive / consolidation / KC 打通）
- **阶段 3（持续）**：超越特性（fuzzy entity / emergent schema / federated）

**总：2-3 个月 MVP，6 个月打磨。**

---

## 8. 待决议问题（写 DESIGN.md 前要想清楚）

1. **Entity node 和现有 memory node 的关系** — 新建一层？还是统一？
2. **Fact 的存储形式** — 继续存自然语言 core_fact？还是升级成 (subject, predicate, object) 三元组？还是两者并存？
3. **Extract pipeline 怎么改** — 单次抽取 vs 两阶段 shallow/deep？
4. **Retrieval 怎么改** — 向量 + Hebbian 之外，加 structured query？Hybrid search 具体实现？
5. **API 边界** — 对外暴露什么？`store`, `recall`, `query`, `reason`？
6. **和 GID 的关系** — 借鉴哪些？共享哪些底层？还是完全独立实现？
7. **Schema 机制** — 预定义 edge types 还是 emergent？还是可插拔的 schema？
8. **多信号融合权重** — 怎么调参？每个信号权重怎么定？
9. **Migration 策略** — 老 engram 用户（含 rustclaw 自己）怎么升级？

---

## 9. 下一步

1. **我去详细研究** mem0 / A-MEM / Letta / LightRAG 的实现细节
2. **产出**：一份对比表（各自怎么做的 entity / edge / retrieval / consolidation）
3. **然后**：基于真实对比写 DESIGN-v0.3.md 草稿
4. **再然后**：设计评审 → 开始阶段 1 实施

---

## 10. 核心原则（贯穿始终）

- **质量第一**：不为省钱牺牲 accuracy
- **神经科学特色保住**：ACT-R / somatic / interoceptive / consolidation 是差异化
- **硬能力必须补**：entity / relation / precision 不能再缺
- **独立可用**：不依赖外挂（不需要"再建一个 KG 层"）
- **嵌入式**：SQLite-first，部署简单
- **渐进式演化**：不是推倒重来，是扩展 + 整合
