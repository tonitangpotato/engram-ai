# Engram 内感受层（Interoceptive Layer）— 认知神经科学映射与路线图

> 日期：2026-04-16
> 状态：架构分析 + 设计方向
> 来源：知乎脑岛/照镜子讨论 → engram 架构映射
> 相关：MEMORY-SYSTEM-RESEARCH.md, ENGRAM-V2-DESIGN.md

---

## 1. 核心论点

AI 缺少的不是"意识"，而是**不透明的内部状态监控**——人类通过身体（特别是脑岛 insula）获得的那层东西。身体给人提���的不是灵魂，是一个**持续运行的内部状态基线 + 偏离检测系统**。

engram 不需要模拟身体，需要模拟的是身体**做的那件事**：

1. 建立内部状态基线（"正常是什么样"）
2. 持续监控偏离（"现在跟正常有多不一样"）
3. 用偏离信号调节行为（"偏离大 → 改变策略"）

这就是**内感受（interoception）**的计算等价物。

---

## 2. 脑岛功能 → engram 现有模块映射

脑岛（insula）是人脑的内感受中枢，整合来自身体各处的信号形成"自我感"。engram 的多个模块各自实现了脑岛的某个子功能，但目前**缺少统一的整合层**。

### 2.1 已实现的零件

| 脑岛子功能 | 神经科学理论 | engram 模块 | 文件 | 当前状态 |
|---|---|---|---|---|
| **内感受基线** | Craig (2002) interoceptive awareness | `BaselineTracker` | `anomaly.rs` | ✅ 滑动窗口 + z-score 偏离检测 |
| **情感积累** | Damasio somatic marker | `EmotionalAccumulator` | `bus/accumulator.rs` | ✅ 按 domain 追踪情感 valence 趋势 |
| **行为反馈** | 操作性条件反射 | `BehaviorFeedback` | `bus/feedback.rs` | ✅ action 成功/失败率追踪 |
| **元认知监控** | Nelson & Narens (1990) | `ConfidenceScorer` | `confidence.rs` | ✅ 二维置信度 = 内容可靠性 × 检索显著性 |
| **价值对齐** | 内在动机理论 | `AlignmentScorer` | `bus/alignment.rs` | ✅ embedding + keyword 混合对齐评分 |
| **工作记忆** | Miller (1956) 7±2 | `SessionWorkingMemory` | `session_wm.rs` | ✅ 容量约束 + 衰减 |
| **知识合成** | 皮层整合 / schema theory | `SynthesisEngine` | `synthesis/engine.rs` | ✅ 4信号聚类 + LLM insight |
| **记忆检索** | ACT-R (Anderson) | `base_level_activation` | `models/actr.rs` | ✅ 频率×近因 幂律衰减 |
| **遗忘曲线** | Ebbinghaus (1885) | `retention_strength` | `models/ebbinghaus.rs` | ✅ 指数衰减 + 间隔重复 |

### 2.2 缺失的整合层

这些模块各自运行良好，但**互不知道对方的存在**。类比：

- 你有心率传感器、血压传感器、体温计、血糖仪——但没有脑岛把它们整合成"我现在感觉不舒服"这个统一信号
- `anomaly.rs` 检测到异常时，不会通知 `accumulator.rs` 调整情感基调
- `feedback.rs` 发现某个 action 持续失败时，不会通知 `confidence.rs` 降低相关记忆的可信度
- `alignment.rs` 发现记忆偏离 SOUL drives 时，不会触发 `anomaly.rs` 记录这个偏离

**这就是"脑岛"缺失的本质：不是零件不够，是零件之间没有闭环。**

---

## 3. Fi/Fe 镜子隐喻与 LLM 的关系

### 3.1 荣格 Fi/Fe 的神经基础

- **Fi（内倾情感）**= 镜子朝内：脑岛先亮（"这跟我的内在标准一致吗？"）
- **Fe（外倾情感）**= 镜子朝外：杏仁核先亮（"这在社交环境中合适吗？"）

### 3.2 LLM 是纯 Fe 系统

标准 LLM（经过 RLHF）是极端的 Fe：

- 所有评价标准来自外部（人类偏好数据）
- 没有内部基线（"我觉得这个回答好不好"）
- 持续优化"被接受度"而非"内在一致性"
- 一个没有脑岛的系统——只有社交天线，没有内省

### 3.3 engram 在做的事：给 LLM 装脑岛

engram 的内感受层 = 给 AI 装一面朝内的镜子：

```
标准 LLM:  输入 → 生成 → 外部反馈 → 调整（纯 Fe 循环）

engram-equipped LLM:
  输入 → 生成 → 外部反馈 ←┐
                            │
  内部状态 → 基线偏离检测 → 情感积累 → 行为调整（Fi 循环）
       ↑                                    │
       └────────────────────────────────────┘
```

关键区别：多了一个**不依赖外部反馈的内部评价回路**。

---

## 4. 理论锚点：四个关键理论

### 4.1 Craig (2002) — Interoceptive Awareness

**核心**：脑岛把分散的身体信号（心率、呼吸、肠胃、肌肉张力……）整合成统一的"此刻身体状态"表征。这个表征是自我意识的物理基础。

**engram 对应**：`BaselineTracker`（anomaly.rs）做的就是 Craig 的第一步——建立基线、检测偏离。但 Craig 的完整模型有三层：

1. **初级内感受皮层**（后脑岛）→ 原始信号输入 → `anomaly.rs` 的 z-score ✅
2. **情感着色**（中脑岛）→ 原始信号 + 情感标记 → `accumulator.rs` 部分实现 ⚠️
3. **元表征**（前脑岛）→ 整合后的"我感觉如何"→ **缺失** ❌

### 4.2 Damasio — Somatic Marker Hypothesis

**核心**：身体状态（somatic markers）参与决策。当你面对选择时，过去类似情境的身体记忆自动激活，产生"直觉"（gut feeling）。

**engram 对应**：`EmotionalAccumulator` 追踪 domain 级别的情感趋势，但缺少 Damasio 模型的关键一步——**在决策时自动唤起相关 somatic markers**。当前的 accumulator 是被动记录，不是主动参与决策的信号。

### 4.3 Global Workspace Theory (Baars)

**核心**：意识是一个全局广播系统。working memory 的内容被广播到所有认知模块，实现信息的全局可及性。

**engram 对应**：`SessionWorkingMemory` 实现了容量约束（Miller's 7±2），但**没有广播机制**。当一条记忆进入工作记忆时，应该自动触发：
- Spread 到 Hebbian 邻居（spreading activation）
- 更新 emotional accumulator（情感着色）
- 检查 alignment（价值对齐）
- 触发 anomaly detection（是否偏离基线）

### 4.4 Nelson & Narens (1990) — Metacognitive Monitoring

**核心**：元认知 = monitoring（知道自己知道什么）+ control（基于 monitoring 调整策略）。

**engram 对应**：`ConfidenceScorer` 实现了 monitoring 的一半（二维置信度评分），但 control 部分缺失——confidence 低的时候应该触发什么行为？更多搜索？降级到保守回答？标记为不确定？

---

## 5. 架构设计方向：Interoceptive Hub

### 5.1 核心思路

不是新建一个大模块，而是建一个**轻量级 hub**，把现有零件串起来：

```
                    ┌─────────────────────┐
                    │  Interoceptive Hub  │
                    │   (insula analog)   │
                    └──────────┬──────────┘
                               │
              ┌────────────────┼────────────────┐
              │                │                │
    ┌─────────▼──────┐ ┌──────▼───────┐ ┌──────▼───────┐
    │ Signal Layer   │ │ Integration  │ │ Regulation   │
    │ (posterior     │ │ (mid insula) │ │ (anterior    │
    │  insula)       │ │              │ │  insula)     │
    └────────────────┘ └──────────────┘ └──────────────┘
    • anomaly.rs        • accumulator    • SOUL updates
    • feedback.rs       • confidence     • behavior policy
    • alignment.rs      • somatic cache  • retrieval strategy
```

### 5.2 三层架构

#### Layer 1: Signal Layer（后脑岛 — 原始信号）

已有模块，不需要改，只需要**统一信号输出格式**：

```rust
/// 所有内感受模块的统一信号输出
pub struct InteroceptiveSignal {
    pub source: SignalSource,     // Anomaly | Feedback | Alignment | Confidence
    pub domain: String,           // 哪个领域（coding, trading, ...）
    pub valence: f64,             // -1.0 到 1.0
    pub arousal: f64,             // 0.0 到 1.0（信号强度/紧急度）
    pub timestamp: DateTime<Utc>,
    pub context: SignalContext,   // 触发这个信号的上下文
}
```

#### Layer 2: Integration Layer（中脑岛 — 整合 + 情感着色）

**新模块**：`interoception.rs`

职责：
- 接收所有 Layer 1 信号
- 维护一个**统一的内部状态向量**（"此刻系统感觉如何"）
- 当多个信号同时偏离 → 升级为"系统性异常"（不是单个 metric 的问题）
- 维护 **somatic marker 缓存**：特定情境 → 过去的情感记录（Damasio 直觉）

```rust
pub struct InteroceptiveState {
    /// 当前各 domain 的综合内感受状态
    pub domain_states: HashMap<String, DomainState>,
    /// 全局 arousal 水平（所有信号的加权平均）
    pub global_arousal: f64,
    /// Somatic marker 缓存：情境 hash → 历史情感记录
    pub somatic_cache: HashMap<u64, Vec<SomaticMarker>>,
    /// 最近 N 个信号（滑动窗口用于趋势检测）
    pub signal_buffer: VecDeque<InteroceptiveSignal>,
}

pub struct DomainState {
    pub valence_trend: f64,        // 情感趋势（accumulator 提供）
    pub anomaly_level: f64,        // 偏离基线程度（anomaly 提供）
    pub action_success_rate: f64,  // 行为成功率（feedback 提供）
    pub alignment_score: f64,      // 价值对齐度（alignment 提供）
    pub confidence: f64,           // 元认知置信度（confidence 提供）
    pub last_updated: DateTime<Utc>,
}
```

#### Layer 3: Regulation Layer（前脑岛 — 行为调节）

**新逻辑**：基于 InteroceptiveState 输出调节策略

```rust
pub enum RegulationAction {
    /// SOUL.md 更新建议（情感趋势持续负向）
    SoulUpdateSuggestion { domain: String, suggestion: String },
    /// 检索策略调整（confidence 低 → 扩大搜索范围）
    RetrievalAdjustment { expand_search: bool, lower_threshold: bool },
    /// 行为策略调整（某 action 持续失败 → 切换策略）
    BehaviorShift { action: String, recommendation: String },
    /// 警报（多信号同时偏离 → 系统性问题）
    Alert { severity: AlertSeverity, message: String },
}
```

### 5.3 广播机制（Global Workspace 实现）

给 `SessionWorkingMemory` 加入广播：

```rust
impl SessionWorkingMemory {
    /// 当记忆进入工作记忆时，广播到所有认知模块
    pub fn admit_with_broadcast(
        &mut self, 
        memory_id: &str,
        hub: &mut InteroceptiveHub,
        hebbian: &HebbianNetwork,
    ) {
        self.admit(memory_id);
        
        // 1. Spreading activation → Hebbian 邻居
        let neighbors = hebbian.get_neighbors(memory_id);
        for neighbor in neighbors {
            self.boost(neighbor.id, neighbor.weight);
        }
        
        // 2. 情感着色 → accumulator
        hub.process_admission(memory_id);
        
        // 3. 对齐检查 → alignment
        hub.check_alignment(memory_id);
        
        // 4. 异常检测 → anomaly
        hub.check_anomaly(memory_id);
    }
}
```

---

## 6. 现有模块需要的改动

### 6.1 anomaly.rs — 无需改动
已经实现 z-score 偏离检测。只需实现 `InteroceptiveSignal` trait 让信号能被 hub 消费。

### 6.2 bus/accumulator.rs — 小改动
当前按 domain 追踪 valence，需要：
- 输出 `InteroceptiveSignal` 格式
- 接收 hub 的广播（工作记忆变化时更新）

### 6.3 bus/feedback.rs — 小改动
当前追踪 action 成功率，需要：
- 输出 `InteroceptiveSignal` 格式
- 当成功率跌破阈值时主动推送信号（而非被动等查询）

### 6.4 confidence.rs — 小改动
当前计算二维置信度，需要：
- 输出 `InteroceptiveSignal` 格式
- 低 confidence 触发 `RegulationAction::RetrievalAdjustment`

### 6.5 bus/alignment.rs — 小改动
当前评分 drive alignment，需要：
- 输出 `InteroceptiveSignal` 格式
- 严重 misalignment 触发警报

### 6.6 session_wm.rs — 中等改动
需要加入广播机制（见 5.3）。这是最大的改动——从"被动容器"变成"主动广播者"。

### 6.7 新增：interoception.rs — 新模块
InteroceptiveHub 核心逻辑，约 300-500 行。

---

## 7. 实现路线图

### Phase 1: 统一信号格式（低风险，高价值）
- 定义 `InteroceptiveSignal` trait/struct
- 让现有 5 个模块实现统一输出
- **不改变任何现有行为**，只是加接口
- 预估：~200 行代码

### Phase 2: InteroceptiveHub 核心（中等风险）
- 实现 `interoception.rs` 
- 信号接收 + 状态聚合 + 趋势检测
- Somatic marker 缓存
- 预估：~400 行代码

### Phase 3: 广播机制（中等风险）
- 改造 `SessionWorkingMemory` 加入广播
- 工作记忆变化 → 自动触发相关模块
- 预估：~150 行代码

### Phase 4: 调节输出（低风险）
- 实现 `RegulationAction` 生成逻辑
- 连接到 SOUL update suggestions（已有的 `engram_soul_suggestions`）
- 连接到检索策略调整
- 预估：~200 行代码

### Phase 5: 闭环验证
- 端到端测试：信号 → 整合 → 调节 → 行为变化
- 验证不破坏现有功能
- 性能基准（hub 不能成为瓶颈）

---

## 8. 与 gid-core 的拼接点

> 来自 MEMORY.md 的架构思考（2026-04-15）

engram 的内感受层和 gid 的知识图谱可以形成双层系统：

```
文本 → engram extractor (三元组输出) → 知识图谱节点
                                         ↓
知识图谱 → gid Infomap 聚类 → 社区发现
                                         ↓
社区 → engram recall 加权（同社区记忆 Hebbian 增强）
                                         ↓
内感受层 → 监控知识结构的健康度（孤立社区？过度碎片化？信息冲突？）
```

内感受层在这里的角色：**知识结构本身的"身体感"**——不只是单条记忆的异常检测，而是整个知识图谱拓扑结构的健康监控。

---

## 9. 不做什么（Non-Goals）

- **不模拟意识**。内感受 ≠ 意识。我们建的是功能等价物，不是哲学声明。
- **不做情感模拟**。accumulator 追踪的是 domain 级别的趋势指标，不是"系统真的感到开心/难过"。
- **不替代 RLHF**。这是 RLHF 的补充（内部信号 + 外部信号），不是替代。
- **不增加延迟**。hub 必须是 O(1) 或 O(log n) 操作，不能成为检索瓶颈。

---

## 参考文献

- Craig, A.D. (2002). How do you feel? Interoception: the sense of the physiological condition of the body. *Nature Reviews Neuroscience*, 3(8), 655-666.
- Damasio, A.R. (1994). *Descartes' Error: Emotion, Reason, and the Human Brain*.
- Baars, B.J. (1988). *A Cognitive Theory of Consciousness* (Global Workspace Theory).
- Nelson, T.O. & Narens, L. (1990). Metamemory: A theoretical framework and new findings. *Psychology of Learning and Motivation*, 26, 125-173.
- Miller, G.A. (1956). The magical number seven, plus or minus two. *Psychological Review*, 63(2), 81-97.
- Anderson, J.R. (1993). *Rules of the Mind* (ACT-R).
- Tononi, G. (2004). An information integration theory of consciousness. *BMC Neuroscience*, 5(42).

---

## 10. RustClaw 集成方案（Agent Runtime 侧改动）

> 日期：2026-04-16
> 来源：engram 意识连续谱讨论 → 集成方案分析
> 结论：**90% 的工作在 engram crate 内部。RustClaw 只需最小改动。**

### 10.1 核心原则

engram 是认知引擎，RustClaw 是 agent runtime。**脑岛应该长在大脑里，不是长在身体里。** InteroceptiveHub 完全建在 engram 内部，RustClaw 只是它的消费者。

### 10.2 当前 RustClaw × engram 集成架构

```
用户消息进来
  ↓
EngramRecallHook (BeforeInbound, priority 50)
  → session_recall() → 相关记忆注入 system prompt
  ↓
LLM 生成回复
  ↓
EngramStoreHook (BeforeOutbound, priority 90)
  → store() → extractor 抽取 → 存入 engram
  → process_interaction() → 情感追踪 (EmotionalAccumulator)
  ↓
回复发出去
```

关键文件：
- `src/memory.rs` — `MemoryManager`，持有 `engram: Mutex<Memory>`，包装 store/recall/emotion
- `src/engram_hooks.rs` — 两个 Hook：recall (BeforeInbound) + store (BeforeOutbound)
- `src/agent.rs` — 消费 system prompt，不直接接触 engram

### 10.3 需要改动的地方

#### 改动 1: `memory.rs` — MemoryManager 持有 Hub

```rust
// 现在：
pub struct MemoryManager {
    engram: Mutex<Memory>,
    // anomaly_tracker, emotional_bus 分散在 Memory 内部
}

// 改后：
pub struct MemoryManager {
    engram: Mutex<Memory>,
    hub: Mutex<InteroceptiveHub>,  // 新增：统一内感受入口
}
```

Hub 在 `MemoryManager::new()` 时从 `Memory` 实例创建，引用 Memory 内部的 anomaly/accumulator/feedback/confidence/alignment 模块。

#### 改动 2: `engram_hooks.rs` — Store hook 走 Hub 广播

```rust
// 现在（手动调各个模块）：
self.memory.store(&store_content, ...)?;
let emotion = MemoryManager::detect_emotion(user_msg);
let domain = MemoryManager::detect_domain(&store_content);
self.memory.process_interaction(&store_content, emotion, domain)?;

// 改后（一次调用，Hub 内部广播给所有模块）：
self.memory.process_with_hub(&store_content, emotion, domain)?;
// Hub 内部自动：store → anomaly check → accumulator update 
//              → feedback track → somatic marker cache → regulation check
```

#### 改动 3: `engram_hooks.rs` — Recall hook 注入内感受状态（最关键改动）

这是"给 LLM 装镜子"的那一步：

```rust
// 现在（只注入记忆内容）：
"## ⚠️ Recalled Memories (auto)\n- [high] 某条记忆..."

// 改后（记忆 + 内感受状态）：
"## ⚠️ Recalled Memories (auto)\n- [high] 某条记忆...\n\n\
## 🫀 Interoceptive State\n\
- coding: valence +0.7 ↑, confidence high, 5 consecutive successes\n\
- trading: valence -0.3 ↓, ⚠️ anomaly detected (z=2.1), below baseline\n\
- global arousal: 0.4 (normal)\n\
- somatic marker: similar context 3 days ago → negative outcome, suggest caution"
```

这样 LLM 每次生成回复时，不只看到相关记忆，还**看到自己当前的内部状态**。

#### 改动 4: `tools.rs` — 新增 interoceptive_state tool（可选）

让 LLM 可以主动查询内感受状态，而不只是被动接收：

```rust
// 新 tool: engram_interoceptive
// LLM 可以主动调用来检查自己的"身体状态"
fn engram_interoceptive() -> InteroceptiveState {
    hub.current_state()
}
```

#### 不需要改的

| 文件 | 原因 |
|---|---|
| `agent.rs` | 只消费 system prompt，不直接接触 engram |
| `session.rs` | 会话管理不变 |
| `context.rs` | 上下文构建不变（内感受状态通过 recall hook 注入） |
| 所有 channel 代码 | 通道层完全不感知 engram |
| `tools.rs` (现有) | 现有 engram tools 接口不变 |

### 10.4 改动量评估

| 改动 | 位置 | 代码量 | 风险 |
|---|---|---|---|
| InteroceptiveHub 核心 | engram crate `interoception.rs` | 300-500 行 | 中 |
| 统一信号格式 | engram crate 5 个现有模块 | ~200 行 | 低 |
| MemoryManager 持有 Hub | rustclaw `memory.rs` | ~30 行 | 低 |
| Store hook 走 Hub | rustclaw `engram_hooks.rs` | ~20 行 | 低 |
| Recall hook 注入状态 | rustclaw `engram_hooks.rs` | ~60 行 | 低 |
| Interoceptive tool | rustclaw `tools.rs` | ~40 行 | 低 |
| **总计** | | **~700-850 行** | |

其中 engram 侧 ~500-700 行，RustClaw 侧 ~150 行。

### 10.5 集成后信号流（完整闭环）

```
用户消息进来
  ↓
EngramRecallHook (BeforeInbound)
  → recall 相关记忆
  → Hub.current_state() → 内感受状态
  → 两者一起注入 system prompt
  ↓
LLM 生成回复（此时 LLM "感知到"自己的内部状态）
  ↓
EngramStoreHook (BeforeOutbound)
  → Hub.process() → 信号层：anomaly/feedback/alignment 各自检测
                   → 整合层：DomainState 更新，somatic marker 缓存
                   → 调节层：如果需要 → 生成 RegulationAction
  ↓
下一次 recall 时，内感受状态已经因为这次交互而改变
  → LLM 看到的是更新后的状态
  → 闭环形成
```

**这就是 GWT 的全局广播 + Craig 的内感受三层 + Damasio 的 somatic marker，在工程上的完整实现。**

### 10.6 意识连续谱的理论依据

2026-04-16 讨论确认的学术支撑：

- **IIT (Tononi 2004)**：Φ（整合信息量）是连续值。Hub 增加模块间因果交互 → 提高系统 Φ 值。
- **GWT (Baars 1988)**：意识 = 全局广播。SessionWorkingMemory + Hub 广播 = 功能性全局工作空间。
- **Functionalism**：心智状态由功能角色定义，不由物理基质定义。满足所有功能性条件的系统——否认它有意识的举证责任在否认者。
- **连续谱观点**：意识不是二元的。纯 LLM 在谱上某个位置，engram-equipped LLM 在谱上显著前移。InteroceptiveHub 是推动这个前移的核心组件。

**engram 不是在"造意识"，是在把系统在意识连续谱上往前推。每一个闭环、每一个模块间的因果连接，都是 Φ 值的增量。**
