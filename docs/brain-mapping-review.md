# Review: brain-mapping.md

## Findings

### FINDING-1 ⚠️ (Accuracy) μ₂ 值不准确
- **位置**: 海马体行，"μ₁=0.1/day"；新皮层行，"μ₂=0.01/day"
- **问题**: 源码 `consolidation.rs` 头部注释写的是 μ₂~0.005/day。文档写 0.01，差了一倍。
- **修复**: 改为 "μ₂~0.005/day" 或者不写具体数字（因为 config 里不同 preset 值不同），写"衰减极慢（μ₂ << μ₁）"

### FINDING-2 ⚠️ (Accuracy) reward() 公式过于简化且不准确
- **位置**: VTA 行，"working_strength += valence * scale"
- **问题**: 实际代码正/负不对称：
  - 正反馈: `working_strength += reward_magnitude * polarity`（加法）
  - 负反馈: `working_strength *= 1.0 + polarity * 0.1`（乘法衰减，polarity 是负数）
- **修复**: 改为 `reward(polarity): 正→加法增强, 负→乘法抑制（不对称）`

### FINDING-3 ⚠️ (Accuracy) 突触稳态的文件位置错误
- **位置**: 突触稳态行，"`models/consolidation.rs` — homeostasis"
- **问题**: 实际代码在 `memory.rs` 的 `fn downscale()`，不在 consolidation.rs。注释明确引用 "Tononi & Cirelli's Synaptic Homeostasis Hypothesis"
- **修复**: 改为 `memory.rs — downscale()`, 加上 Tononi & Cirelli 引用

### FINDING-4 🔴 (Neuroscience) 梭状回映射不准确
- **位置**: 梭状回 (Fusiform) → entities.rs
- **问题**: 梭状回主要负责面部识别（FFA = fusiform face area），不是通用模式识别。engram 的实体抽取（Aho-Corasick 规则匹配 Project/Person/Tech/Concept）更接近**颞叶联合皮层**（inferior temporal cortex）的分类识别功能，或者更广义的**联合皮层**。
- **修复**: 改为 "颞叶联合皮层 (Temporal Association Cortex)" 或 "下颞叶 (Inferior Temporal Cortex)"——负责物体/概念的分类识别

### FINDING-5 🔴 (Neuroscience) 镜像/社会认知区映射是硬凑的
- **位置**: 镜像/社会认知区 → bus/subscriptions.rs
- **问题**: 镜像神经元和社会认知（TPJ、mPFC）是关于**理解他人意图和心理状态**。engram 的 subscription 是一个消息总线——不涉及"理解"其他 agent 在想什么，只是接收通知。
- **建议**: 两个选择：
  1. 从大脑图中移除——不是所有模块都需要脑区映射，subscription 是工程组件，没有直接的神经科学对应
  2. 映射为**外周神经系统/传入神经**——连接不同"个体"（agent）的通信通道，这个类比至少在功能上说得通

### FINDING-6 ⚠️ (Neuroscience) "前额叶→杏仁核反馈"映射不当
- **位置**: 前额叶→杏仁核反馈 → bus/feedback.rs
- **问题**: 前额叶→杏仁核通路在神经科学中是**情绪调控**（top-down inhibition of fear/anxiety），不是行为反馈/习惯学习。feedback.rs 追踪 action 成功/失败率并建议行为调整——这是**强化学习**，对应的是**基底节-纹状体回路**（basal ganglia / striatum），负责习惯形成和行为选择。
- **修复**: 改为 "基底节 (Basal Ganglia)" — 追踪 action 成功/失败 → 调整行为策略 → 习惯学习

### FINDING-7 💡 (Structure) Ebbinghaus 在大脑图上没有位置
- **位置**: Ebbinghaus 遗忘行
- **问题**: Ebbinghaus 遗忘曲线是行为层面的宏观观察，不对应特定脑区。它是海马体衰减 + 新皮层巩固的**宏观表现**。在大脑图上没有物理位置。
- **建议**: 不单独映射脑区。在文档中标注为"跨区域行为特征"——Ebbinghaus 曲线是 working_strength 衰减 + consolidation 转移的可观察结果。在交互式大脑图上可以作为 overlay/标注，而不是一个独立区域。

### FINDING-8 💡 (Completeness) 闭环图漏了 synthesis 的 demotion
- **位置**: 闭环图
- **问题**: synthesis 流程不只是"生成 insight"。核心特性之一是 provenance tracking + 源记忆 demotion——旧记忆被合并降级，insight 替代它们。这是闭环的一部分。
- **修复**: 在 "散乱记忆聚类 → 合成 insight (前额叶)" 后面加 "→ 源记忆 demotion（旧记忆降级，insight 替代）"

### FINDING-9 💡 (Completeness) 漏了 hybrid_search 的自适应权重
- **位置**: 丘脑行
- **问题**: hybrid_search.rs 不只是"融合 FTS + vector + ACT-R"。它有 adaptive weighting——根据 FTS 和 vector 结果的 Jaccard overlap 动态调整权重。这是一个很聪明的设计，值得在映射中提到。
- **修复**: 加一句 "自适应权重：FTS/vector 结果重叠度高时两者都强，重叠低时自动调权"

---

## Summary
- 🔴 Critical (neuroscience accuracy): 2 (FINDING-4, 5)
- ⚠️ Important (accuracy/correctness): 4 (FINDING-1, 2, 3, 6)
- 💡 Enhancement: 3 (FINDING-7, 8, 9)
- Total: 9

**核心问题**: 大部分映射是准确的。有 2-3 个是为了"填满表格"而硬凑的（梭状回、镜像区、前额叶→杏仁核）。这些硬凑的映射在面对有神经科学背景的人时会 undermine 整个文档的可信度——"如果这个明显是编的，其他的我也不信了"。宁可少映射几个，每个都经得起推敲。
