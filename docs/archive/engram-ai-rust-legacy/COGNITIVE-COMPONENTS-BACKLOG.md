# Engram — 认知神经科学组件 Backlog

> 日期：2026-04-16
> 状态：已整理进 `issues-index.md`（ISS-011~013 + FEAT-001~004）
> 目的：记录所有讨论过但尚未开始设计/实现的认知科学组件（理论背景和详细描述）
> 来源：INTEROCEPTIVE-LAYER.md, ENGRAM-V2-DESIGN.md, MEMORY-SYSTEM-RESEARCH.md, cognitive-autoresearch Doc 02
> **跟踪**: 实现状态统一在 `.gid/docs/issues-index.md` 的 FEAT-001~004 部分管理，本文档保留理论细节

---

## 当前 engram 已实现的认知模块（基准线）

| 模块 | 源文件 | 认知理论 | 状态 |
|------|--------|---------|------|
| ACT-R 激活衰减 | `models/actr.rs` | Anderson ACT-R | ✅ 完整 |
| Hebbian 关联学习 | `models/hebbian.rs` | Hebb's Rule | ✅ 完整（但无因果方向） |
| Ebbinghaus 遗忘曲线 | `models/ebbinghaus.rs` | Ebbinghaus 1885 | ✅ 完整 |
| CLS 记忆巩固 | `models/consolidation.rs` | Complementary Learning Systems | ✅ 完整 |
| 异常/惊讶检测 | `anomaly.rs` | Prediction Error / Bayesian Surprise | ✅ 完整 |
| 元认知置信度 | `confidence.rs` | Metacognitive Monitoring | ✅ 完整 |
| 工作记忆 | `session_wm.rs` | Miller's 7±2 / Baddeley WM | ✅ 完整 |
| 情感总线 | `bus/` | Distributed Cognition | ✅ 基础框架 |

**半成品（有代码但不完整）：**

| 模块 | 源文件 | 缺什么 |
|------|--------|--------|
| 奖励信号 | `memory.rs::reward()` | 只有极性×magnitude，不是预测误差信号 |
| 情绪标记 | `bus/feedback.rs` | 各模块独立运作，无全局协同响应 |
| SHY 突触缩放 | `memory.rs::downscale()` | 等比例缩放，无周期性自动触发 |
| 自适应搜索 | `hybrid_search.rs` | 仅搜索权重自适应，非全局参数自适应 |

---

## 未实现组件一览

### A. 内感受层（来源：INTEROCEPTIVE-LAYER.md）

这些组件构成一个统一的自我监控系统——让 engram 知道"自己现在整体状态如何"。

| # | 组件 | 描述 | 认知理论 | 依赖 | 复杂度 |
|---|------|------|---------|------|--------|
| A1 | **InteroceptiveSignal 统一格式** | 所有内部模块（anomaly、emotion、WM压力、知识健康度）输出统一信号格式 | Craig's Interoception | 无 | 低 |
| A2 | **InteroceptiveHub 中枢** | 整合所有信号，维护"身体状态"快照，检测模式 | Anterior Insula / Craig三层 | A1 | 中 |
| A3 | **GWT 全局广播** | SessionWM 变化 → 自动广播到所有模块，实现"意识"级联 | Global Workspace Theory (Baars) | A2, bus/ | 中 |
| A4 | **Somatic Marker 缓存** | 情境→直觉映射，基于历史情绪经验快速决策 | Damasio Somatic Marker | A2, emotion bus | 中 |
| A5 | **调节输出层** | RegulationAction：SOUL 更新建议、检索策略调整、行为模式切换 | Allostasis / Predictive Processing | A2 | 低 |
| A6 | **Craig 中脑岛** | 跨模态整合（多个信号流合并成统一感受） | Craig's Mid-Insula | A1, A2 | 中 |
| A7 | **Craig 前脑岛** | 元意识——"我知道我现在的状态"，自我模型 | Craig's Anterior Insula | A6 | 高 |

### B. 情感闭环（来源：ENGRAM-V2-DESIGN.md）

这些组件把 engram 从"被动存储"变成"主动循环系统"——情绪影响记忆，记忆影响行为，行为产生新情绪。

| # | 组件 | 描述 | 当前状态 | 复杂度 |
|---|------|------|---------|--------|
| B1 | **Emotion → SOUL 闭环** | 持续负面情绪 → 自动建议 SOUL.md 更新 | accumulator 有，闭环无 | 中 |
| B2 | **SOUL → Engram importance** | SOUL 驱动自动调整记忆 importance 权重 | alignment scorer 有，自动调权无 | 低 |
| B3 | **Engram → HEARTBEAT 自适应** | 记忆状态驱动 heartbeat 行为（如：异常多→增加巡检） | 无 | 低 |
| B4 | **HEARTBEAT → Engram 经验回流** | heartbeat 发现 → 作为经验存入 engram | 无 | 低 |
| B5 | **IDENTITY 自动演化** | 基于积累的经验和情感趋势，自动更新 IDENTITY.md | 无 | 中 |
| B6 | **Voice 情感分析** | 语音特征（语速、音调、能量）→ 情绪标签 | 无 | 中 |

### C. 记忆生命周期（来源：MEMORY-SYSTEM-RESEARCH.md）

这些组件把 engram 从"存了就存了"变成"记忆会演化、去重、合成、结构化"。

| # | 组件 | 描述 | 当前状态 | 复杂度 |
|---|------|------|---------|--------|
| C1 | **Gate 入口过滤** | 输入信息 → 决定是否值得记忆 | ✅ 已完成（hook filter + content filter + extractor negatives） | 中 |
| C2 | **Mission-Steered Extraction** | 用 SOUL.md 驱动引导提取，不无脑存一切 | 无 | 中 |
| C3 | **Embedding Reconciler** | 新记忆 vs 已有记忆 → embedding 距离去重/合并 | 无 | 中 |
| C4 | **LLM Reconciler** | 矛盾记忆 → LLM 决定保留哪个/如何合并 | 无 | 高 |
| C5 | **Observation Consolidation** | 多条观察 → 合成为一条知识（"见过5次X→X是事实"） | synthesis 部分覆盖 | 中 |
| C6 | **Entity Graph** | 实体关系网络（Person→worksAt→Company） | schema 有，代码零（ISS-009） | 高 |
| C7 | **Multi-Retrieval Fusion** | TEMPR 级检索（时间+实体+语义+元数据+图多路融合） | ✅ 已实现 | 高 |
| C8 | **Working Context State Machine** | 对话上下文的状态管理（不只是最近N条） | 无 | 中 |
| C9 | **Activation Heatmap** | 可视化记忆激活分布，调试 + 洞察 | 无 | 低 |

### D. 认知科学理论（来源：cognitive-autoresearch Doc 02）

这些是 Doc 02 列出的认知理论，engram 内部文档尚未讨论过如何在推理层实现。它们也是 Doc 08（从推理到训练）的前置依赖。

| # | 组件 | 认知理论 | 描述 | 潜在实现方向 | 复杂度 |
|---|------|---------|------|-------------|--------|
| D1 | **STDP 因果方向** | Spike-Timing-Dependent Plasticity | Hebbian 只知道"A和B相关"，STDP 知道"A导致B" | 给 Hebbian link 加时序方向和因果权重 | 中 |
| D2 | **神经调质全局调控** | Neuromodulation (DA/5-HT/ACh/NE) | 四种调质信号全局调制所有模块行为（专注/探索/学习率/警觉） | 4个连续变量 → 调制所有模块的参数 | 高 |
| D3 | **感觉门控/丘脑过滤** | Sensory Gating / Thalamic Filter | 输入层过滤——不是所有信息都能进入"意识"，低相关的被丘脑挡掉 | 输入预处理层，基于当前焦点过滤 | 中 |
| D4 | **稀疏编码** | Sparse Distributed Representation | 不是所有记忆都同时活跃，只有少数高度相关的被激活 | top-k 激活 + 抑制非相关记忆 | 中 |
| D5 | **神经振荡** | Neural Oscillations (Theta/Gamma) | 周期性计算模式——theta 节奏绑定记忆编码，gamma 绑定感知 | 周期性 consolidation/recall 节奏 | 高 |
| D6 | **小世界网络** | Small-World Network Topology | 记忆不是扁平存储，有 hub 节点和短路径，信息传播效率高 | Hebbian graph → 小世界拓扑优化 | 高 |
| D7 | **皮层柱** | Cortical Column / Minicolumn | 相关记忆组成功能单元，单元内竞争+单元间协作 | 记忆分组 + 组内 winner-take-all | 高 |
| D8 | **时间细胞** | Time Cells (Hippocampal) | 对数时间编码——"这件事发生在大约3天前"而不是精确时间戳 | 对数时间桶 + 时间模糊化检索 | 中 |

### E. 两边都提到但 engram 还没做的（交叉区）

这些是最高优先级——cognitive-autoresearch 和 engram 内部设计文档都认为需要，但代码为零。

| # | 组件 | Doc 02 | engram 内部文档 | 代码 | 为什么重要 |
|---|------|--------|----------------|------|-----------|
| E1 | GWT 全局广播 | ✅ | ✅ INTEROCEPTIVE A3 | ❌ | 所有模块联动的基础设施 |
| E2 | Somatic Marker | ✅ | ✅ INTEROCEPTIVE A4 | ❌ | 快速直觉决策，减少 LLM 调用 |
| E3 | 元认知 Control | ✅ | ✅ INTEROCEPTIVE A7 | ❌ | "我知道我不知道"——自我校准 |
| E4 | 神经调质 | ✅ | ⚠️ V2 暗含 | ❌ | 全局调制信号，串联所有模块 |

---

## 统计

- **已实现**: 8 个完整 + 4 个半成品 = 12 个模块有代码
- **未实现总计**: A(7) + B(6) + C(9) + D(8) + E(4 去重后不额外计) = **30 个组件待实现**
- **交叉区（最高优先级）**: 4 个（E1-E4）

---

## 实现建议（仅排序思路，非正式 plan）

**如果要挑最有影响力的先做：**

1. **D1 STDP** — 改动最小（Hebbian 已有，加方向），收益最大（因果推理）
2. **A1 统一信号格式** — 所有内感受组件的前置依赖，且代码量极小
3. **C3 Embedding Reconciler** — 当前最大痛点（重复记忆），用户可感知
4. **E1 GWT 广播** — 解锁所有模块联动，但需要 A1 先完成
5. **D8 时间细胞** — 独立模块，不影响其他，但改善时间相关检索

**最难啃的骨头（需要深度设计后再动）：**
- D2 神经调质（影响所有模块参数）
- D6 小世界网络（需要重构 Hebbian graph 存储）
- D7 皮层柱（需要全新的记忆组织方式）
- C4 LLM Reconciler（需要 LLM 调用，成本和延迟问题）

---

## 变更日志

- 2026-04-16: 初始版本，从 4 个来源文档汇总
