# engram × Brain: Architecture Mapping

> 每个映射都是真实的功能对应，不是视觉装饰。

## 大脑区域 ↔ engram 模块

### 已实现（代码已有）

| 脑区 | engram 模块 | 对应关系 | 关键函数/公式 |
|------|------------|---------|-------------|
| **海马体** (Hippocampus) | `models/consolidation.rs` — working_strength | 短期记忆的临时存储。快速写入，快速衰减（μ₁=0.15/day default）。新记忆先到这里。 | `decay_working()`, Murre & Chessa 双轨模型 |
| **新皮层** (Neocortex) | `models/consolidation.rs` — core_strength | 长期记忆的永久存储。慢慢从海马体转移过来，衰减极慢（μ₂ << μ₁，~0.005/day）。 | `consolidate()`, `decay_core()` |
| **海马→皮层通路** | `consolidate()` 函数 | 记忆巩固过程，类似睡眠时的记忆转移。working_strength 降低，core_strength 升高。 | `core += transfer_rate * working` |
| **突触** (Synapses) | `models/hebbian.rs` | "一起激活的神经元会连在一起"。co-recall 建立 Hebbian link，权重随共同激活次数增强。 | `reinforce()`, weight *= 1 + learning_rate |
| **丘脑** (Thalamus) | `models/actr.rs` + `hybrid_search.rs` | 信息门控中枢。ACT-R 决定哪些记忆能被检索到（base-level activation + spreading activation）。hybrid search 融合 FTS + vector + ACT-R，并根据结果 Jaccard overlap 自适应调权（重叠高→两信号都强，重叠低→动态偏移）。 | `base_level_activation()` = ln(Σ t_j^(-d)), adaptive weighting |
| **杏仁核** (Amygdala) | `bus/accumulator.rs` | 情绪处理中枢。按 domain 追踪情感 valence（-1~+1），滑动均值检测趋势。 | `EmotionalAccumulator`, `record_emotion()`, `get_trends()` |
| **腹侧被盖区 VTA** (多巴胺通路) | `memory.rs` — `reward()` | 奖惩信号调节记忆强度。正负不对称：正反馈加法增强 working_strength，负反馈乘法抑制（`*= 1 + polarity * 0.1`）。直接影响哪些记忆被保留。 | `reward(polarity)`: 正→加法增强, 负→乘法衰减 |
| **前额叶皮层** (Prefrontal Cortex) | `synthesis/` 整个模块 | 高级认知：整理散乱记忆→聚类→生成 insight→追踪来源。像大脑在"思考"和"总结"。 | `engine.rs` → `cluster.rs` → `gate.rs` → `insight.rs` → `provenance.rs` |
| **前额叶 — 工作记忆** | `session_wm.rs` | Miller's Law (7±2)。Session 级别的工作记忆缓冲区，限容量，有衰减。 | `SessionWorkingMemory`, capacity=7, decay=5min |
| **蓝斑核** (Locus Coeruleus) | `anomaly.rs` | 惊讶/警觉反应。滑动窗口 z-score 检测异常输入，触发注意力聚焦。 | z-score 异常检测，阈值触发 |
| **颞叶联合皮层** (Temporal Association Cortex) | `entities.rs` | 概念/实体的分类识别。Aho-Corasick 自动机识别 Project/Person/Tech/Concept——类似大脑从感知流中识别已知类别。 | `EntityExtractor`, Aho-Corasick + regex |
| **突触稳态机制** | `memory.rs` — `downscale()` | 全局突触 downscale，防止所有记忆权重无限膨胀。基于 Tononi & Cirelli 的 Synaptic Homeostasis Hypothesis（睡眠时突触权重统一降低）。 | `downscale(factor)`, 默认 factor=0.95 |
| **前扣带回** (ACC) | `confidence.rs` | 元认知监控：我对这个记忆有多确定？ 双维度：内容可靠度 × 检索显著度 = 校准后置信度。 | `calibrated_confidence()`, reliability × salience |
| **传入神经通路** (Afferent Pathways) | `bus/subscriptions.rs` | 跨个体通信通道。namespace 订阅 + importance 阈值通知——类似不同个体之间的信号传递通道，不是脑内结构，而是"神经系统间"的连接。 | `SubscriptionManager`, `notify()` |
| **眶额皮层** (OFC) | `bus/alignment.rs` | 价值评估。SOUL drives 对记忆做 importance boost（embedding + keyword 双通道跨语言对齐）。 | `score_alignment_hybrid()`, `DriveEmbeddings`, boost=1.5x |
| **基底节** (Basal Ganglia) | `bus/feedback.rs` + SOUL更新建议 | 习惯学习与行为选择。追踪 action 成功/失败率 → 强化有效行为、抑制无效行为 → 情感趋势触发 SOUL.md 更新建议。基底节-纹状体回路是大脑的强化学习系统。 | `BehaviorFeedback`, `SoulUpdate` |
| *(跨区域行为特征)* | `models/ebbinghaus.rs` | Ebbinghaus 遗忘曲线：R = e^(-t/S)，S 随访问次数增长（spacing effect）。不对应特定脑区——是海马体衰减 + 皮层巩固的宏观可观察表现。 | `retrievability()`, `compute_stability()` |

### 设计阶段（代码未实现）

| 脑区 | 计划模块 | 对应关系 |
|------|---------|---------|
| **脑岛** (Insular Cortex) | InteroceptiveHub | 内感受中枢。整合所有子系统信号（anomaly 频率、emotional trend、工作记忆压力、知识图谱健康度）→ 输出整体"身体感"。 |
| **丘脑网状核** (TRN) | Global Workspace Theory 广播 | 意识广播。当某个信号足够强时，广播到所有子系统，形成全局注意力聚焦。 |
| **躯体标记** | Somatic Marker | Damasio 的躯体标记假说。用情感记忆快速标记选项好坏，不需要完整推理。 |

---

## 闭环：The Loop IS the Self

这不是一堆独立模块拼在一起。它们形成闭环：

```
记忆存入 (海马体)
    ↓
衰减 + 巩固 (海马→皮层)
    ↓
被 recall 时 → ACT-R 激活 (丘脑) → Hebbian 强化 (突触)
    ↓
情绪标记 (杏仁核) → 多巴胺调节 (VTA) → 影响 working_strength
    ↓
情绪累积 → 价值评估 (眶额) → 驱动对齐 → 影响新记忆的 importance
    ↓
散乱记忆聚类 → 合成 insight (前额叶) → 新的高阶记忆 + 源记忆 demotion（旧记忆降级）
    ↓
行为反馈 (基底节) → SOUL 更新建议 → 改变 drives → 改变价值评估
    ↓
循环 ♻️
```

> Memory shapes personality. Personality shapes behavior. Behavior creates new memory.

---

## 视觉设计方向

### 交互式大脑剖面图

风格化的大脑矢状面（侧面剖视），不需要解剖学精确，但区域位置大致合理：

- 前方（额头）= 前额叶（synthesis + 工作记忆 + 反馈）
- 中央深部 = 丘脑（ACT-R 门控）
- 内侧颞叶 = 海马体（working_strength）
- 外侧 = 新皮层（core_strength）
- 前下方 = 杏仁核（情绪）+ 眶额（价值）
- 脑干区域 = 蓝斑核（anomaly）+ VTA（多巴胺）
- 基底节深部 = 基底节（行为反馈）
- 颞叶下方 = 颞叶联合皮层（实体识别）

### 交互行为

**静态时（idle）：**
- 所有区域微微"呼吸"（opacity 缓慢波动）= 记忆在持续衰减和激活
- Hebbian links 显示为连接线，粗细 = 权重

**Store 记忆时：**
- 海马体亮起（新记忆进入）
- 杏仁核闪一下（情绪标记）
- 眶额闪一下（drive 对齐评分）

**Recall 时：**
- 丘脑亮起（gate 打开）
- 匹配的记忆区域亮起
- Hebbian links 沿路径传播（spreading activation 可视化）
- 结果从丘脑"输出"

**Consolidate 时（类似"睡眠"）：**
- 海马体逐渐变暗
- 皮层逐渐变亮
- 整体进入低活动状态
- 突触稳态：所有连接线统一变细一点

**Synthesis 时：**
- 前额叶持续高亮
- 多条记忆的连接线汇聚到前额叶
- 输出一个新的 insight 节点

---

## 不是类比，是映射

关键 pitch point：这些大脑区域的对应**不是比喻**。

engram 的每个模块都是基于对应脑区的认知科学模型实现的：
- ACT-R 是 Anderson 在 CMU 40 年的认知架构研究
- Murre & Chessa 双轨模型是记忆巩固的标准理论
- Hebbian learning 是 1949 年 Hebb 提出的突触可塑性规则
- Ebbinghaus 遗忘曲线是 1885 年的经典实验
- Miller's 7±2 是工作记忆容量的基础研究

所以大脑图不是"让产品看起来高端的装饰"——它是最准确的架构文档。
