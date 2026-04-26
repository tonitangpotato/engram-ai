# ISS-019 战略 Pivot — 2026-04-22

## 与 potato 的对齐

**终极目标**：让 engram 在 **LoCoMo benchmark Local Mode** 上提分。

**当前 LoCoMo 状态**：
- ~2000 questions, accuracy ~77.9%
- **48.3% 的错误**来自"engram 召回 10 条 memory，无一相关"
- 根因：activation level 作为排序主信号，与 query relevance 不相关
- 诊断结论：**missing type-aware retrieval**

## 认知演进（三次修订）

### 第一次认知（错误）
Step 9 smoke pilot 只是 QA 环节，优化它是 yak shaving，应该跳过直接做 ISS-020。

### 第二次认知（部分错误）
开 ISS-020 Phase A baseline 测量代替 Step 9，让真实 LoCoMo 数据同时充当覆盖率验证。

### 第三次认知（基于 pilot log 实际数据）
**Step 9 的两次 run 实际已经产出关键诊断信号：**

| Source | stored% | participants | temporal | causation | dim 容器 |
|---|---|---|---|---|---|
| engram/telegram (100) | 94.0% | **0.0%** ❌ | 6.1% ❌ | 13.3% ❌ | 100% ✅ |
| sessions (40) | 62.5% | 65.6% ✅ | 75.0% ✅ | **6.2%** ❌ | 100% ✅ |

**这揭示了一个此前未知的 bug：子维度抽取能力在不同内容类型上差异巨大。**

- `engram.dimensions` 容器 100% 覆盖（Step 1-8 的写路径修复生效）
- 但 participants/temporal/causation **子字段稀疏且不稳定**
- 这意味着 **ISS-020 的 dim-aware ranking 前提不成立**——没有 dim 值可匹配

## ISS-019 的价值链条（重新对齐 v3）

```
写路径容器漏洞修复（Step 1-8） ✅ 已完成
  ↓
子维度抽取覆盖率（Step 9 发现不达标）⚠️ 需要新 issue 诊断
  ↓
维度齐全 + 覆盖率达标
  ↓
读路径 dim-aware ranking（ISS-020）
  ↓
LoCoMo 48.3% "retrieved-but-irrelevant" 误差下降
  ↓
accuracy 提升（ablation 量化）
```

## 执行调整

| 项目 | 状态 | 说明 |
|---|---|---|
| Step 1-8 写路径修复 | ✅ done | 容器层 100% 覆盖已验证 |
| Step 9 smoke pilot | ✅ done-diagnostic | 不是"失败"，是"成功揭示下游问题"。关闭 issue 但保留代码和 log |
| Step 9 task #1（断言/诊断分离） | ❌ cancelled | Step 9 诊断任务已完成，不需要重构 |
| Step 9 task #3（切数据源） | ❌ cancelled | 前提不成立（engram DB 无 raw_memory 表），且 Step 9 已跑过 sessions 源 |
| Step 10 full rebuild | ⏸️ blocked | 等 ISS-021 解决子维度抽取问题后才值得做 |
| **ISS-021 子维度抽取覆盖率诊断** | 🆕 new | 回答：为什么 participants/temporal/causation 子字段稀疏？ |
| ISS-020 LoCoMo dim-aware retrieval | 🆕 new | 依赖 ISS-021，等弹药 ready 后才做 |

## ISS-021 草案（新开）

**目标**：回答"为什么 Step 9 显示子维度稀疏？"

**假设空间**：
- **H1** LegacyClassification dispatch 漏：部分路径没触发 v2 分类器
- **H2** Extractor 不抽：participants/temporal/causation 没有对应的抽取逻辑
- **H3** 丢字段：抽到了但序列化到 metadata 时被丢弃（另一个写路径漏点）
- **H4** 内容真稀疏：任务类文本本身没人名/时间/因果，这是内容本质限制

**初步代码审查发现（2026-04-22）**：

`src/extractor.rs:176-180` 的 extractor prompt 显式写着：
```
"participants": "Who was involved (omit if not mentioned)",
"temporal": "When it happened (omit if not mentioned)",
"causation": "Why it happened / motivation (omit if not mentioned)",
```

→ **部分命中 H2+H4 混合**：
- extractor **有**抽取逻辑
- 但 schema 是 "sparse by design"——"omit if not mentioned"
- LLM 忠实执行了 → telegram 单用户消息很少有 participants
- sessions 多轮对话有参与者 → 所以 participants 66%
- causation 需要显式因果 → 两源都稀疏

**这不是 bug，是 prompt 设计选择与 dim-aware ranking 需求之间的语义 mismatch：**
- 当前语义："字段缺失 = 文本没提到"
- ranking 需要：区分"没提到"与"真的无关"两种缺失
- 或：允许 LLM 在"没明确提到但可推理"时填入维度

**决策点（需要 potato 拍板）**：
- **方向 A**：保持 extractor 现状，改 ranking 把"维度缺失"当中性信号处理
- **方向 B**：改 extractor prompt，要求从上下文推断维度（如单用户消息 → participants = 发言者）
- **方向 C**：双 extractor 策略——现有"保守抽取"为主，ranking 时用"LLM 即席推断"做查询侧增强

**实验（待方向确定后执行）**：
1. 从 Step 9 target DB 随机抽 10 条"维度为空"的记忆，人工标注"原文里是否有可推断的 participants/temporal/causation 信息"
2. 量化 H4 vs H2 比例：如果 >70% 的"缺失"原文里真的没有可抽取信息 → H4 为主 → 选方向 A；否则 H2 为主 → 方向 B
3. 根据 LoCoMo 对话数据 sample 预测覆盖率（LoCoMo 是多人对话，应该天然比 telegram 高）

## ISS-020 草案（依赖 ISS-021）

**Phase A: Baseline 测量**
1. ISS-021 结案后重新灌 LoCoMo memories
2. SQL 检查维度覆盖率（目标：participants/temporal/causation 各 >60%）
3. 跑 LoCoMo baseline

**Phase B: Dim-aware ranking 接入**
4. 检索时调用 `extract_query_dimensions(query)` 得到 query dims
5. 对 candidate memory 计算 dim-match score
6. 加入排序函数（与 activation、embedding 相似度加权）

**Phase C: Ablation**
7. 对比 activation-only vs dim-aware
8. 量化 Δaccuracy

## 教训

**今天犯的认知错误**：在提出"Step 9 是 yak shaving"时，没先看 Step 9 已产生的数据。那些"失败断言"本身就是宝贵的诊断信号，不是"Step 9 设计有问题"。

**教训**：在判断一个任务是不是 yak shaving 之前，**先看它的输出**。输出空才是真 yak shaving；输出是关键诊断信号就不是。

## 签字

- 初议：RustClaw（13:31 ET）
- 确认：potato（13:31 ET "对的"）
- 再议（读 log 后）：RustClaw（13:45 ET）
- 再确认：potato（13:46 ET "好的你决定吧"）
