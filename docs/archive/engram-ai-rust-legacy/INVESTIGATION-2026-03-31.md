# Engram 生产环境问题调查报告

> 调查日期：2026-03-31
> 调查环境：RustClaw v0.1.0 (engramai 0.2.2 as Rust crate dependency)
> 数据库：`/Users/potato/rustclaw/engram-memory.db` (1,434 条记忆)

---

## 1. 架构现状

### RustClaw 如何使用 Engram

RustClaw 通过 **Rust crate 依赖**（不是 CLI）使用 engramai：

```toml
# Cargo.toml
engramai = "0.2.2"
```

数据流：

```
用户消息
  ↓
┌─────────────────────────────────────────────────┐
│ EngramRecallHook (BeforeInbound)                │
│   src/engram_hooks.rs:29-70                     │
│   → memory.recall(user_message) → 注入 context  │
└─────────────────────────────────────────────────┘
  ↓
Claude 处理消息，生成回复
  ↓
┌─────────────────────────────────────────────────┐
│ EngramStoreHook (BeforeOutbound)                │
│   src/engram_hooks.rs:130-145                   │
│   → 拼接 "用户消息 → 回复前200字"              │
│   → memory.store(content, source="auto")        │
│     → 内部调用 Haiku extractor 提取结构化 facts │
│     → 存入 SQLite (source="auto-extract")       │
└─────────────────────────────────────────────────┘
```

另外，Agent 工具层（`src/tools.rs`）提供三个手动工具：
- `engram_store` — Agent 主动调用存储（source="agent_tool"）
- `engram_recall` — Agent 主动查询
- `engram_recall_associated` — 关联记忆查询

### Extractor 配置

```rust
// src/memory.rs:83-96
// 使用 Anthropic Haiku 做 LLM extraction
let extractor = AnthropicExtractor::with_token_provider(
    token_provider, false  // not thinking mode
);
engram.set_extractor(Box::new(extractor));
```

Haiku 收到原始文本后，用 `extractor.rs` 中的 prompt 提取结构化 facts，返回 `ExtractedFact` 数组（每个包含 content, memory_type, importance, source）。

---

## 2. 问题一：垃圾记忆写入

### 2.1 现象

数据库中存在大量低价值、重复的记忆：

```
190+ 条 heartbeat 相关垃圾
多次重复的系统指令被当作 procedural knowledge
整段 LLM 输出（包含表格、状态报告）被原样存储
```

### 2.2 按来源分布

| Source | 条数 | 说明 |
|--------|------|------|
| `auto-extract` | 689 | Haiku extractor 提取的结构化 facts |
| `auto` | 407 | EngramStoreHook 的原始存储 |
| `stdp:auto` | 115 | STDP 自动关联 |
| `(empty)` | 107 | 来源字段为空的旧数据 |
| `agent_tool` | 63 | Agent 手动存储（质量最好） |
| 其他 | ~53 | MEMORY.md 导入、手动等 |
| **总计** | **1,434** | |

### 2.3 根因分析

#### 根因 1: EngramStoreHook 不区分消息类型

**位置**：`src/engram_hooks.rs` 第 130-145 行

`BeforeOutbound` hook 对**所有** agent 回复触发存储，包括：
- ✅ 正常用户对话（应该存）
- ❌ Heartbeat session 的状态检查回复（不应该存）
- ❌ NO_REPLY 回复（不应该存）
- ❌ 系统性/重复性操作（不应该存）

Heartbeat 每 30 分钟触发一次，每次都把 HEARTBEAT.md 的指令和状态报告存入 engram。几周下来堆积了 **104 条** source=`auto` 的 heartbeat 垃圾。

**样本**：
```
"Read HEARTBEAT.md if it exists (workspace context). Follow it strictly. 
Do not infer or repeat old tasks from prior chats. If nothing needs attention, 
reply HEARTBEAT_OK. → HEARTBEAT_OK

| Check | Status |
| **MM Bot** | ✅ Running (PID 2824824)..."
```

#### 根因 2: Haiku Extractor 无法区分"系统指令"和"知识"

**位置**：`engramai/src/extractor.rs`

Extractor prompt 要求 Haiku 从文本中提取 "important facts, relationships, preferences, and insights"。但它没有被告知：
- 系统指令（"Read HEARTBEAT.md..."）不是知识
- 状态检查报告不是值得记住的事实
- 已经存过的内容不需要再存

结果 Haiku 把 heartbeat 指令提取成了 procedural memory，importance 标到 0.7-0.9：

```
"Should not infer or repeat old tasks from prior chats unless specified in HEARTBEAT.md" (procedural, 0.9)
"System should reply HEARTBEAT_OK when nothing requires attention" (procedural, 0.8)
"Reply with HEARTBEAT_OK when nothing requires attention" (procedural, 0.8)
"HEARTBEAT.md file should be checked at the start of conversations" (procedural, 0.9)
```

同一条指令被存了 **十几次**（每个 heartbeat session 一次）。

#### 根因 3: 没有去重机制

`memory.rs` 的 `add()` 方法直接插入 SQLite，不检查是否已存在相似内容。

同一个 heartbeat 指令每 30 分钟被存一次，几天累积出大量完全相同的记忆。

### 2.4 垃圾类型明细

| 类型 | 估计条数 | 来源 | 示例 |
|------|---------|------|------|
| Heartbeat 指令重复 | ~50 | auto + auto-extract | "Should follow HEARTBEAT.md strictly..." |
| 状态报告原文 | ~30 | auto-extract | "MM Bot running, balance $81.33..." |
| 系统错误日志 | ~20 | auto-extract | "[ESCAPED] System: Exec failed..." |
| "System has N memories" | ~15 | auto | "System has 1213 memories stored" |
| 磁盘空间报告 | ~10 | auto | "System disk has 56GB free space" |
| 空洞的指令提取 | ~65 | auto-extract | "User should check HEARTBEAT.md..." |
| **总计** | **~190** | | **占总量 13%** |

---

## 3. 问题二：Recall 准确率下降

### 3.1 现象

recall 返回的结果中经常混入无关信息，真正有价值的记忆被挤掉。

### 3.2 根因分析

#### 因素 1: 噪声稀释（Signal-to-Noise Ratio 下降）

190+ 条垃圾占总量 13%。ACT-R 排序时这些垃圾也参与竞争。

关键问题：heartbeat procedural 垃圾的 importance 被标为 **0.7-0.9**（Haiku 认为这些是重要的操作指令），而很多真正有价值的记忆 importance 只有 0.6。在 ACT-R 模型中 importance 直接影响 activation score，导致垃圾反而排在前面。

#### 因素 2: 重复记忆的频率偏好

ACT-R 的 base-level activation 公式：`B_i = ln(Σ_k t_k^{-d})`

每次 access 都增加 activation。同一条 heartbeat 指令被存了十几条，每条都有 access_log，累积 activation 远高于只被存一次的正常记忆。

recall("HEARTBEAT") 会返回 5 个几乎一样的结果，浪费所有 recall slots。

#### 因素 3: FTS5 关键词匹配 vs 语义相关性

当前 recall 同时使用 FTS5 全文搜索和 ACT-R activation。FTS5 是关键词匹配，不是语义搜索（除非配了 embedding）。

查询 "AI phone agent" 时，FTS5 匹配 "phone" 和 "agent" 两个词，可能也匹配到 heartbeat 报告中出现的 "agent" 一词，噪声进一步放大。

### 3.3 量化影响

假设 recall limit=5，正常应返回 5 条高质量记忆：
- 有噪声时：可能 2-3 条被垃圾占据 → 有效信息量降 40-60%
- Heartbeat procedural 记忆 activation 偏高 → 越来越容易被召回 → 恶性循环

---

## 4. 修复建议

### 4.1 RustClaw 侧修复（消费者侧）

#### Fix 1: EngramStoreHook 加过滤器

```rust
// src/engram_hooks.rs - BeforeOutbound
fn should_store(content: &str, metadata: &HashMap<String, String>) -> bool {
    // 跳过 heartbeat session
    if metadata.get("session_type") == Some(&"heartbeat".to_string()) {
        return false;
    }
    // 跳过 NO_REPLY / HEARTBEAT_OK
    let trimmed = content.trim();
    if trimmed == "NO_REPLY" || trimmed == "HEARTBEAT_OK" {
        return false;
    }
    // 跳过太短的回复（通常是 ack）
    if trimmed.len() < 20 {
        return false;
    }
    true
}
```

#### Fix 2: 标记 heartbeat session 的 metadata

在 heartbeat channel 发送消息时，附加 `session_type: "heartbeat"` metadata，让 hook 能识别。

### 4.2 Engram 侧修复（engramai crate）

#### Fix 3: Extractor 加 negative examples

在 `extractor.rs` 的 prompt 中添加：

```
Do NOT extract:
- System instructions or operational directives (e.g., "Read HEARTBEAT.md", "reply HEARTBEAT_OK")
- Status check results (e.g., "System has N memories", "disk has N GB free")
- Error logs or escaped system messages
- Repetitive operational patterns that aren't novel information
```

#### Fix 4: 存储前去重

在 `memory.rs` 的 `add()` 方法中加 dedup 检查：

```rust
pub fn add(&mut self, content: &str, ...) -> Result<Vec<String>> {
    // Dedup: 查 FTS5 是否已有 >90% 相似的内容
    let existing = self.recall(content, 1, None, None)?;
    if let Some(top) = existing.first() {
        if similarity(content, &top.content) > 0.9 {
            // 只更新 access_log，不创建新记忆
            self.log_access(&top.id)?;
            return Ok(vec![top.id.clone()]);
        }
    }
    // 正常插入...
}
```

或者更轻量的方案：content hash 去重。

#### Fix 5: Importance 校准

Haiku extractor 提取的 importance 需要有上限/基线校准。建议：
- Auto-extracted facts 的 importance 上限 0.7（不能高于手动存储的基线）
- Procedural memory 的 importance 默认 0.5（除非内容明显是用户偏好/工作流）

#### Fix 6: Recall 结果去重

`recall()` 返回结果时做 post-processing dedup：

```rust
// 如果两条结果 content 相似度 > 0.8，只保留 activation 更高的那条
fn dedup_results(results: Vec<RecalledMemory>) -> Vec<RecalledMemory> {
    // ...
}
```

### 4.3 数据清理

清理现有 1,434 条记忆中的垃圾：

```sql
-- 删除重复的 heartbeat 指令（保留最新的 1 条）
DELETE FROM memories WHERE rowid NOT IN (
    SELECT MAX(rowid) FROM memories 
    WHERE content LIKE '%HEARTBEAT%' 
    GROUP BY content
) AND content LIKE '%HEARTBEAT%';

-- 删除状态报告
DELETE FROM memories WHERE content LIKE 'System has % memories%'
    OR content LIKE 'System disk has%'
    OR content LIKE 'System stats are normal%';

-- 删除原样存储的 heartbeat 回复
DELETE FROM memories WHERE content LIKE 'Read HEARTBEAT.md if it exists%'
    AND source = 'auto';

-- 重建 FTS 索引
INSERT INTO memories_fts(memories_fts) VALUES('rebuild');
```

预计清理 ~190 条，保留 ~1,244 条。

---

## 5. 优先级排序

| 优先级 | 修复 | 位置 | 影响 | 工作量 |
|--------|------|------|------|--------|
| P0 | EngramStoreHook 过滤 heartbeat | RustClaw | 阻止新垃圾写入 | 30min |
| P0 | 清理现有垃圾数据 | SQL | 立即改善 recall 质量 | 15min |
| P1 | Extractor 加 negative examples | engramai crate | 提高提取质量 | 1h |
| P1 | Recall 结果去重 | engramai crate | 避免 5 个结果全一样 | 1h |
| P2 | 存储前去重 | engramai crate | 防止长期退化 | 2h |
| P2 | Importance 校准 | engramai crate | 更合理的排序 | 1h |

---

## 6. 长期思考

### 6.1 Auto-store 的设计缺陷

根本问题：**所有 agent 回复都值得记住**这个假设是错误的。

更好的设计：让 Agent（Claude）自己决定什么值得存，而不是 hook 无脑存一切。Agent 已经有 `engram_store` 工具可以手动存储——也许 auto-store 应该**完全关闭**，改为在 system prompt 中指导 Agent 主动存重要信息。

对比：
- Auto-store: 低延迟，高噪声，Haiku token 成本
- Agent 主动存: 更精准，但依赖 Agent 自觉性，可能遗漏

建议折中：Auto-store 只存 importance > 0.7 的对话（由长度、是否包含决策/偏好等启发式规则判断），其余交给 Agent 主动。

### 6.2 Embedding 的缺失

当前 recall 主要依赖 FTS5（关键词匹配）+ ACT-R（时间衰减 + importance）。没有 embedding 层意味着：
- 跨语言查询效果差（"AI phone agent" 查不到中文记忆）
- 语义近义词匹配差（"doctor appointment" 查不到 "约医生"）

engramai v0.2.2 支持 `hybrid_search.rs`（keyword + embedding + ACT-R 混合），但需要配置 embedding provider。RustClaw 目前**没有配 embedding provider**（只配了 Haiku extractor），所以 recall 完全走 FTS5 + ACT-R。

这是 recall 准确率不高的另一个隐藏因素。

### 6.3 STDP 噪声

`stdp:auto` 有 115 条，是 Spike-Timing Dependent Plasticity 自动创建的关联记忆。这些是 Hebbian 学习的副产品——当两个记忆频繁被一起召回时自动创建因果链接。需要验证这些关联的质量。

---

## 附录：数据样本

### A. 典型垃圾记忆（auto-extract）

```
"Should not infer or repeat old tasks from prior chats unless specified in HEARTBEAT.md"
  → type: procedural, importance: 0.9, source: auto-extract

"System should reply HEARTBEAT_OK when nothing requires attention"
  → type: procedural, importance: 0.7, source: auto-extract

"System has 1213 memories stored"
  → type: factual, importance: 0.6, source: auto
```

### B. 典型有价值记忆（agent_tool）

```
"AI Phone Agent 交互模式重大调整：主入口是消息渠道（Telegram/WhatsApp），不是 Web UI..."
  → type: factual, importance: 1.0, source: agent_tool

"potato 在2026年3月31日讨论了AI Phone Agent Platform模块的当前状态和代码组织"
  → type: episodic, importance: 0.5, source: agent_tool
```

### C. 可疑的 auto 存储

```
"Read HEARTBEAT.md if it exists... → Running django-16408, P2P=18 but F2P=0"
  → type: (raw), importance: 0.6, source: auto-extract
  → 这是 SWE-bench 的状态报告，跟当前工作完全无关
```
