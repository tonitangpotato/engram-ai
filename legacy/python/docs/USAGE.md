# engramai 使用指南

*完整的集成和使用文档*

---

## 目录

1. [安装](#安装)
2. [基础使用](#基础使用)
3. [LLM 用户指南](#llm-用户指南)
4. [Clawdbot 用户指南](#clawdbot-用户指南)
5. [CLI 命令参考](#cli-命令参考)
6. [Python API 详解](#python-api-详解)
7. [MCP 工具参考](#mcp-工具参考)
8. [配置预设](#配置预设)
9. [最佳实践](#最佳实践)
10. [故障排除](#故障排除)

---

## 安装

### Python

```bash
pip install engramai
```

### TypeScript/Node.js

```bash
npm install neuromemory-ai
```

### 验证安装

```bash
neuromem --version
neuromem stats
```

---

## 基础使用

### 最简示例

```python
from engram import Memory

# 创建/打开记忆数据库
mem = Memory("./my-agent.db")

# 存储记忆
mem.add("用户喜欢简洁的回答", type="relational", importance=0.8)

# 召回记忆
results = mem.recall("用户偏好", limit=5)
for r in results:
    print(f"[{r['confidence_label']}] {r['content']}")

# 日常维护
mem.consolidate()  # 记忆整合
mem.forget()       # 清理弱记忆
```

---

## LLM 用户指南

### Claude Desktop 配置

**1. 确保已安装 engramai**

```bash
pip install engramai
```

**2. 编辑配置文件**

macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
Windows: `%APPDATA%\Claude\claude_desktop_config.json`

```json
{
  "mcpServers": {
    "engram": {
      "command": "python3",
      "args": ["-m", "engram.mcp_server"],
      "env": {
        "ENGRAM_DB_PATH": "/Users/你的用户名/Documents/claude-memory.db"
      }
    }
  }
}
```

**3. 重启 Claude Desktop**

**4. 使用**

Claude 现在可以使用以下工具：
- 存储记忆："记住我喜欢用 Python"
- 召回记忆："你记得我的编程偏好吗？"
- 整合记忆："整理一下你的记忆"

### Cursor 配置

**1. 在项目根目录创建 `.cursor/mcp.json`**

```json
{
  "mcpServers": {
    "engram": {
      "command": "python3",
      "args": ["-m", "engram.mcp_server"],
      "env": {
        "ENGRAM_DB_PATH": "./project-memory.db"
      }
    }
  }
}
```

**2. 重启 Cursor**

### 其他 MCP 客户端

任何支持 MCP 协议的客户端都可以使用 engram。通用配置：

```json
{
  "command": "python3",
  "args": ["-m", "engram.mcp_server"],
  "env": {
    "ENGRAM_DB_PATH": "/path/to/memory.db"
  }
}
```

---

## Clawdbot 用户指南

Clawdbot 是一个 AI agent 平台，支持多种方式集成 engramai。

### 方式一：CLI Skill（最简单）

**安装**

```bash
pip install engramai
clawdhub install engramai
```

**使用**

直接与你的 agent 对话：

| 你说 | Agent 执行 |
|------|-----------|
| "记住我喜欢深色模式" | `neuromem add "用户喜欢深色模式" --type preference` |
| "我之前说过什么关于 Python 的事？" | `neuromem recall "Python"` |
| "整理一下你的记忆" | `neuromem consolidate` |
| "你记了多少东西？" | `neuromem stats` |

这种方式下，agent 通过 CLI 调用 engramai，不需要修改配置。

### 方式二：MCP 集成（更深度）

**编辑 Clawdbot 配置**

`~/.clawdbot/config.yml`:

```yaml
mcp:
  servers:
    engram:
      command: python3
      args: ["-m", "engram.mcp_server"]
      env:
        ENGRAM_DB_PATH: ~/.clawdbot/agents/main/memory.db
```

**重启 Clawdbot**

```bash
clawdbot gateway restart
```

现在你的 agent 可以直接调用 MCP 工具，而不是通过 CLI。

### 方式三：替换原生记忆系统（完全改造）

Clawdbot 默认使用文件系统存储记忆：
- `MEMORY.md` — 长期记忆
- `memory/*.md` — 每日笔记

要用 engramai 完全替换这套系统：

**1. 配置 MCP（同方式二）**

**2. 修改 `AGENTS.md`**

```markdown
## Memory System

使用 engram MCP 工具进行所有记忆操作：

### 存储记忆
当学到重要信息时，调用 `engram.store`：
- type: factual（事实）/ episodic（经历）/ relational（关系偏好）/ procedural（操作流程）
- importance: 0.0-1.0，越重要越高

### 召回记忆
回答问题前，先调用 `engram.recall` 检索相关记忆。

### 日常维护
在 heartbeat 期间调用：
- `engram.consolidate` — 每天一次
- `engram.forget --threshold 0.01` — 清理弱记忆

### 用户反馈
当用户表示满意/不满意时，调用 `engram.reward` 强化/抑制最近记忆。

**不要使用文件系统记忆**（MEMORY.md, memory/*.md）。
```

**3. 迁移现有记忆（可选）**

```python
from engram import Memory

mem = Memory("~/.clawdbot/agents/main/memory.db")

# 读取 MEMORY.md 并导入
with open("MEMORY.md") as f:
    content = f.read()
    # 解析并添加每条记忆
    for line in content.split("\n"):
        if line.strip() and not line.startswith("#"):
            mem.add(line, type="factual", importance=0.7)
```

### 两种记忆系统对比

| 特性 | 文件系统（原生） | engramai |
|------|----------------|----------------|
| 存储 | Markdown 文件 | SQLite 数据库 |
| 遗忘 | 手动清理 | 自动遗忘曲线 |
| 整合 | 无 | Memory Chain Model |
| 关联 | 无 | Hebbian 学习 |
| 检索 | grep/关键词 | ACT-R 激活度排序 |
| 反馈学习 | 无 | 支持 |
| 适合 | 简单场景、调试 | 长期运行、复杂交互 |

你可以根据需要选择任一方式，或者同时使用两者（文件系统用于日志，engram 用于活跃记忆）。

---

## AI Agent 集成模式

这一节详细说明 AI Agent 应该**何时**以及**如何**调用 engram。这是让 engram 正确工作的关键。

### ⚠️ 关键：配置 Agent Habits

**仅安装 MCP 是不够的！** Agent 需要被明确指示何时使用 engram。

将以下内容加入 agent 的系统配置（如 Clawdbot 的 `HEARTBEAT.md` 或 `AGENTS.md`）：

```markdown
## Memory Habits (必须配置)

### 对话中 — 主动使用
1. **回答历史/偏好问题前** → 先 `engram.recall`
2. **学到重要信息时** → 主动 `engram.store`
3. **用户反馈时** → 调用 `engram.reward`

### 定期维护 — Heartbeat/Cron
每天至少一次：
- `engram.consolidate` — 记忆整合
- `engram.forget --threshold 0.01` — 清理弱记忆
```

**为什么这很重要？**
- 工具安装了 ≠ 会自动使用
- Agent 需要显式指令来形成"习惯"
- 没有这些指令，engram 就是一个被遗忘的工具

### 核心原则

engram 不是被动存储，而是**主动记忆系统**。Agent 需要在正确的时机调用正确的操作。

### 调用时机表

| 场景 | 操作 | 说明 |
|------|------|------|
| 学到用户偏好 | `engram.store(type="relational")` | "用户喜欢简洁回答" |
| 学到事实知识 | `engram.store(type="factual")` | "项目用 Python 3.12" |
| 学到操作流程 | `engram.store(type="procedural")` | "部署需要先跑测试" |
| 重要对话发生 | `engram.store(type="episodic")` | "今天讨论了架构重构" |
| 回答涉及历史的问题 | 先 `engram.recall` | 检索相关记忆再回答 |
| 用户表示满意 | `engram.reward("正面反馈")` | 强化最近记忆 |
| 用户表示不满 | `engram.reward("负面反馈")` | 抑制最近记忆 |
| 定期维护 (heartbeat/cron) | `engram.consolidate` | 每天一次 |
| 定期清理 (heartbeat/cron) | `engram.forget` | 每天或每周 |

### 存储策略

**应该存储：**
- ✅ 用户明确表达的偏好
- ✅ 重要的事实信息
- ✅ 学到的教训（"这样做不行"）
- ✅ 关键决定及其原因
- ✅ 用户的工作习惯和模式

**不应该存储：**
- ❌ 每句对话（太多噪音）
- ❌ 临时性信息（"今天天气如何"）
- ❌ 可以随时查询的公开信息
- ❌ 敏感信息（除非明确要求）

### 检索策略

**何时检索：**
```
用户提问 → 判断是否涉及历史/偏好 → 是 → engram.recall → 结合记忆回答
                                   → 否 → 直接回答
```

**检索示例：**
```python
# 用户问："我之前说过喜欢什么编程语言？"
memories = engram.recall("编程语言 偏好", limit=5)
# 用记忆回答

# 用户问："帮我写个 Python 脚本"
# 不需要检索，直接执行
```

### 重要性设置指南

| importance | 适用场景 |
|------------|---------|
| 0.9 - 1.0 | 核心偏好、关键信息（API密钥位置、绝对禁忌）|
| 0.7 - 0.8 | 重要偏好、常用信息（代码风格、项目结构）|
| 0.5 - 0.6 | 一般信息（普通事实、一般经历）|
| 0.3 - 0.4 | 低优先级（闲聊内容、临时记录）|

### 混合模式（推荐）

同时使用 engram 和文件系统，各司其职：

```markdown
## Memory System (Hybrid)

### engram — 主动记忆
- 用途：检索、关联、动态权重
- 存储：偏好、事实、教训
- 操作：store/recall/reward

### 文件系统 — 日志记录
- memory/YYYY-MM-DD.md：每日原始记录
- MEMORY.md：人类可读的长期备份
- 用途：透明、调试、手动编辑
```

### Heartbeat 维护任务

将以下任务加入 agent 的定期执行（heartbeat 或 cron）：

```markdown
## 记忆维护（每天执行）

1. 运行整合
   - 调用 `engram.consolidate`
   - 将工作记忆转移到核心记忆

2. 清理弱记忆
   - 调用 `engram.forget --threshold 0.01`
   - 移除长期未使用的低权重记忆

3. 检查统计
   - 调用 `engram.stats`
   - 确保记忆数量在合理范围（建议 < 1000）
```

### 集成检查清单

在将 engram 集成到你的 agent 之前，确保：

- [ ] MCP server 已配置并运行
- [ ] Agent 的系统提示/配置中包含调用时机说明
- [ ] 定期维护任务已设置（consolidate/forget）
- [ ] 测试 store → recall 流程正常工作
- [ ] 考虑是否需要混合模式（文件 + engram）

---

## CLI 命令参考

```bash
# 全局选项
neuromem --db ./custom.db <command>  # 指定数据库路径
neuromem --help                       # 帮助信息

# 添加记忆
neuromem add "记忆内容" [选项]
  --type TYPE        # factual|episodic|relational|emotional|procedural|opinion
  --importance NUM   # 0.0-1.0
  --source SOURCE    # 来源标识
  --tags TAG1,TAG2   # 标签（逗号分隔）

# 召回记忆
neuromem recall "查询词" [选项]
  --limit NUM        # 返回数量，默认 5
  --types TYPES      # 过滤类型
  --min-confidence NUM

# 列出记忆
neuromem list [选项]
  --limit NUM        # 数量限制
  --type TYPE        # 按类型过滤
  --layer LAYER      # core|working|archive

# 整合记忆
neuromem consolidate [选项]
  --days NUM         # 模拟天数，默认 1

# 清理弱记忆
neuromem forget [选项]
  --threshold NUM    # 强度阈值，默认 0.01
  --id ID            # 删除特定记忆

# 用户反馈
neuromem reward "反馈内容"
  # 正面: "很好" "正确" "完美" → 强化
  # 负面: "错了" "不对" → 抑制

# 统计信息
neuromem stats

# Hebbian 关联
neuromem hebbian "记忆内容或ID"
  # 显示与该记忆相关联的其他记忆

# 导出数据库
neuromem export backup.db
```

---

## Python API 详解

### 初始化

```python
from engram import Memory
from engram.config import MemoryConfig

# 基础初始化
mem = Memory("./agent.db")

# 使用预设配置
mem = Memory("./agent.db", config=MemoryConfig.personal_assistant())

# 带 embedding 支持
from engram.embeddings import SentenceTransformerAdapter
mem = Memory("./agent.db", embedding=SentenceTransformerAdapter())
```

### 存储记忆

```python
mid = mem.add(
    content="用户是一名软件工程师",
    type="relational",      # 记忆类型
    importance=0.8,         # 重要性 0-1
    source="profile",       # 来源标识（可选）
    tags=["user", "career"] # 标签（可选）
)
print(f"Memory ID: {mid}")
```

### 召回记忆

```python
results = mem.recall(
    query="用户的职业是什么",
    limit=5,                           # 返回数量
    context=["career", "background"],  # 上下文关键词（可选）
    types=["relational", "factual"],   # 类型过滤（可选）
    min_confidence=0.3,                # 最低置信度（可选）
    graph_expand=True                  # 启用 Hebbian 关联扩展（可选）
)

for r in results:
    print(f"[{r['confidence_label']}] {r['content']}")
    print(f"  - ID: {r['id']}")
    print(f"  - Type: {r['type']}")
    print(f"  - Confidence: {r['confidence']:.2f}")
    print(f"  - Layer: {r['layer']}")
```

### 记忆维护

```python
# 整合（每天运行一次）
mem.consolidate(days=1)

# 清理弱记忆
mem.forget(threshold=0.01)

# 手动遗忘特定记忆
mem.forget(memory_id="abc123")

# 固定重要记忆（永不遗忘）
mem.pin("abc123")
mem.unpin("abc123")
```

### 反馈学习

```python
# 正面反馈 → 强化最近激活的记忆
mem.reward("太棒了，正是我需要的！")

# 负面反馈 → 抑制最近激活的记忆
mem.reward("不对，这是错误的")
```

### 矛盾处理

```python
# 添加矛盾记忆
old_id = mem.add("数据库在 us-east-1", type="factual")
new_id = mem.add("数据库已迁移到 us-west-2", type="factual", contradicts=old_id)

# 或更新现有记忆
new_id = mem.update_memory(old_id, "数据库已迁移到 us-west-2")

# 召回时，旧记忆会被标记为 contradicted，置信度 ×0.3
```

### 统计信息

```python
stats = mem.stats()
print(f"Total memories: {stats['total']}")
print(f"By layer: {stats['by_layer']}")
print(f"By type: {stats['by_type']}")
```

---

## MCP 工具参考

### engram.store

存储新记忆。

**参数:**
- `content` (string, required): 记忆内容
- `type` (string): factual|episodic|relational|emotional|procedural|opinion
- `importance` (number): 0.0-1.0
- `source` (string): 来源标识
- `tags` (array): 标签列表

**返回:** 记忆 ID

### engram.recall

召回记忆。

**参数:**
- `query` (string, required): 查询词
- `limit` (number): 返回数量，默认 5
- `types` (array): 类型过滤
- `min_confidence` (number): 最低置信度

**返回:** 记忆列表，按 ACT-R 激活度排序

### engram.consolidate

运行记忆整合。

**参数:**
- `days` (number): 模拟天数，默认 1

**返回:** 整合统计

### engram.forget

清理弱记忆。

**参数:**
- `threshold` (number): 强度阈值，默认 0.01
- `memory_id` (string): 删除特定记忆

**返回:** 删除数量

### engram.reward

应用用户反馈。

**参数:**
- `feedback` (string, required): 反馈内容

**返回:** 影响的记忆数量

### engram.stats

获取统计信息。

**返回:** 统计对象

### engram.export

导出数据库。

**参数:**
- `path` (string, required): 导出路径

**返回:** 成功/失败

---

## 配置预设

```python
from engram.config import MemoryConfig

# Chatbot: 高重放率，慢遗忘
mem = Memory("bot.db", config=MemoryConfig.chatbot())

# Task Agent: 快遗忘，专注最近任务
mem = Memory("worker.db", config=MemoryConfig.task_agent())

# Personal Assistant: 长期记忆，关系重要
mem = Memory("assistant.db", config=MemoryConfig.personal_assistant())

# Researcher: 永不遗忘，存档一切
mem = Memory("research.db", config=MemoryConfig.researcher())
```

### 自定义配置

```python
from engram.config import MemoryConfig

config = MemoryConfig(
    working_decay_rate=0.15,     # 工作记忆衰减率
    core_decay_rate=0.002,       # 核心记忆衰减率
    consolidation_threshold=0.3, # 整合阈值
    forget_threshold=0.01,       # 遗忘阈值
    replay_probability=0.3,      # 重放概率
    downscale_factor=0.95,       # 下调因子
)

mem = Memory("custom.db", config=config)
```

---

## 最佳实践

### 1. 选择合适的记忆类型

| 类型 | 使用场景 | 默认衰减 |
|------|---------|---------|
| factual | 事实、知识 | 中 |
| episodic | 事件、经历 | 快 |
| relational | 关系、偏好 | 慢 |
| emotional | 情绪时刻 | 中 |
| procedural | 操作流程 | 很慢 |
| opinion | 观点、信念 | 中 |

### 2. 合理设置重要性

- 0.9-1.0: 关键信息（API 密钥位置、核心偏好）
- 0.7-0.8: 重要信息（用户背景、常用操作）
- 0.4-0.6: 一般信息（普通对话内容）
- 0.1-0.3: 低重要性（随机闲聊）

### 3. 定期维护

```python
# 每天运行
mem.consolidate()
mem.forget(threshold=0.01)

# 每周运行
mem.downscale(factor=0.95)
```

### 4. 利用 Hebbian 学习

频繁一起召回的记忆会自动形成关联：

```python
# 多次一起查询
for _ in range(3):
    mem.recall("Python machine learning", limit=5)

# 之后查询 "Python" 会自动关联 ML 相关记忆
mem.recall("Python tools", graph_expand=True)
```

### 5. 使用反馈学习

```python
# 用户满意时
mem.reward("完美！正是我需要的")

# 用户不满意时
mem.reward("不对，这是错误的")
```

---

## 故障排除

### 数据库锁定

**问题:** SQLite 并发写入时锁定

**解决:** 每个进程使用单独的 Memory 实例，或使用 WAL 模式：

```python
mem = Memory("./agent.db")
mem._store.conn.execute("PRAGMA journal_mode=WAL")
```

### MCP 服务器不启动

**问题:** Claude Desktop 找不到 MCP 服务器

**排查:**
1. 确认 engramai 已安装: `pip show engramai`
2. 确认 python3 路径正确: `which python3`
3. 检查配置文件 JSON 格式是否正确
4. 查看 Claude Desktop 日志

### 记忆检索不准确

**问题:** 召回的记忆不相关

**解决:**
1. 尝试启用 embedding: `pip install engramai[embeddings]`
2. 添加更多上下文关键词到查询
3. 检查记忆类型是否正确

### 记忆消失太快

**问题:** 重要记忆被遗忘

**解决:**
1. 提高记忆的 importance 值
2. 使用 `mem.pin(id)` 固定重要记忆
3. 调整配置中的衰减率

---

## 链接

- **PyPI:** https://pypi.org/project/engramai/
- **npm:** https://www.npmjs.com/package/engramai
- **GitHub:** https://github.com/tonitangpotato/engramai
- **Issues:** https://github.com/tonitangpotato/engramai/issues

---

*有问题？欢迎在 GitHub 提 issue 或贡献代码！*
