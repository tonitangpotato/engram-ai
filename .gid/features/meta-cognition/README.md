# Feature: Meta-Cognition Loop

> 让 engram 从"被动记忆系统"进化为"主动觉察系统" — 在 LLM 行为发生前后插入检查点，利用现有机制（somatic markers、Hebbian learning、anomaly detection）实现类似人类正念觉察的能力。

**Created**: 2026-04-19
**Status**: Planning
**Priority**: P1
**Origin**: potato 从自身 ADHD 觉察训练出发的类比 — 在冲动和行动之间插入间隙

---

## 问题

LLM 有"简化倾向"（statistical shortest-path bias）：省略 edge cases、跳过完整流程、给表面答案。这类似 ADHD 的冲动简化行为。

engram 现有的检测机制（anomaly、confidence、regulation）都在**错误的时间尺度**运作 — 跨 session 统计，事后复盘。缺少的是**行为发生前后的实时检查点**。

## 核心设计

### 类比

```
人类觉察训练:
  刺激 → [间隙/觉察] → 评估 → 选择 → 行动 → 反馈 → 习惯强化

engram meta-cognition:
  Context → [pre_check] → LLM 生成 → [post_check] → 输出 → 反馈 → marker 更新
```

### 两个 Hook

**1. `pre_check(context) → MetaCognitiveAlert`**
- 在 LLM 调用前，用当前 context 做快速 somatic marker recall
- 如果命中已知的失败 pattern → 注入警告到 system prompt
- 示例：`"⚠️ 类似情境下过去出现过简化行为，请确保完整性"`
- 成本：一次 engram recall（~ms 级），不需要额外 LLM 调用

**2. `post_check(context, output) → MetaCognitiveEval`**
- 在 LLM 输出后、发送前，做轻量评估
- 启发式规则：输出长度 vs 问题复杂度比值、confidence score、pattern 匹配
- 如果检测到简化 pattern → 可选择：标记/重新生成/注入补充
- 结果反馈回 engram：Hebbian 链接更新、somatic marker 强化/弱化

### 利用现有机制

| 现有模块 | 在 meta-cognition 中的角色 |
|---|---|
| Somatic Markers | 存储 (situation → outcome) 缓存，pre_check 的核心 |
| Hebbian Learning | 自动加强反复出现的 (pattern → bad_outcome) 关联 |
| Anomaly Detection | post_check 中检测输出偏离 baseline |
| Confidence Calibration | 低 confidence → 触发更严格的 post_check |
| Regulation | 从 pattern 数据产生长期行为建议 |
| InteroceptiveHub | 整合所有信号，提供统一的 meta-cognitive state |

### 学习闭环

```
Phase 1（冷启动）：
  - 没有 markers → 不触发 pre_check
  - post_check 纯规则：长度比、confidence
  - 人类反馈 "这个输出简化了" → 记录为负样本

Phase 2（积累期）：
  - Hebbian links 开始形成
  - Somatic markers 开始命中
  - pre_check 开始注入警告
  - 简化行为减少

Phase 3（自动化）：
  - Markers 足够丰富，大部分简化 pattern 被自动拦截
  - 只有新类型的问题需要 post_check 兜底
  - 类似人类训练后的"自动觉察"
```

## 与持续训练的关系

当前架构：静态 LLM + 动态 engram（外部可塑性模拟）
- engram 在 prompt 层注入觉察信号，LLM 通过 context 接收

未来架构：动态 LLM + 动态 engram
- engram 收集的 pattern 数据 → 训练信号（LoRA/adapter 增量更新）
- Somatic markers → 直接影响 attention weights
- 不再需要 prompt 注入，模型本身具备觉察能力

engram 的角色从"替代可塑性"变成"指导可塑性"。

## 实现路径

1. **定义 `MetaCognitiveCheck` trait** — pre_check / post_check 接口
2. **实现 `pre_check`** — somatic marker recall + alert 生成
3. **实现 `post_check`** — 启发式评估 + 反馈记录
4. **收集 pattern 数据的 API** — 记录"简化行为"事件 + 人类反馈
5. **在 RustClaw agent loop 中集成** — 两个 hook 点
6. **仪表盘/可观测性** — 查看哪些 patterns 被捕获，marker 命中率

## 不做什么

- 不做 LLM 内部的 activation steering（需要模型层访问权限）
- 不做每次都完整 re-generate（成本太高）
- 不做通用"AI 安全"检查（这不是 safety filter，是认知质量）

---

*"觉察不是一个步骤，是一种持续的背景监控。engram 的 somatic markers 就是为此设计的 — 只差把它接到正确的时间点。"*
