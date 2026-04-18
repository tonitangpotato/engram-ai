# Issues: engram-ai-rust (engramai)

> 项目使用过程中发现的 bug、改进点和待办事项。
> 格式: ISS-{NNN} [{type}] [{priority}] [{status}]
> 
> 状态值: `open`, `in_progress`, `closed`, `wontfix`, `blocked`
> 
> 相关文档：
> - `INVESTIGATION-2026-03-31.md` — 生产环境问题深度调查
> - `LEARNINGS.md` — 运维笔记 + recall 质量改进方案
> - `MEMORY-SYSTEM-RESEARCH.md` — 与 Hindsight/Mem0/Zep 对比调研
> - `ENGRAM-V2-DESIGN.md` — 情感总线架构设计
> - `INTEROCEPTIVE-LAYER.md` — 内感受层（脑岛）完整设计
> - `COGNITIVE-COMPONENTS-BACKLOG.md` — 30 个认知组件汇总（A-E 分类）

---

## 📊 Recall 质量提升总结 (2026-04-09)

按**对 recall 质量的实际影响**排序。

### Tier 1 — 结构性天花板（决定 recall 能做到多好）

| # | Issue | 问题 | 状态 |
|---|-------|------|------|
| 1 | **ISS-009** | Entity 索引完全空置 — schema 有、代码零调用、数据零行。recall 无法做概念级跳转 | P1 open |
| 2 | **ISS-005** | Consolidate 不合成知识 — 只加强 activation，不合成 insight | P1 closed |

### Tier 2 — 信噪比（决定 recall 结果有多干净）

| # | Issue | 问题 | 状态 |
|---|-------|------|------|
| 3 | **Phase A1** | EngramStoreHook 无过滤 — heartbeat/NO_REPLY/系统指令全存，垃圾持续写入（上游污染） | ✅ done |
| 4 | **ISS-003** | add() 无 dedup — ~5% 重复率（短期可忍），高频集群挤占排名 | P2 closed |
| 5 | **Phase A2** | Extractor 无 negative examples — Haiku 把系统指令当知识提取 | ✅ done |
| 9 | **ISS-011** | Recall 结果去重 — 返回的 top-K 含近似重复，浪费 context window | P2 open |
| 10 | **ISS-012** | Importance 校准 — auto-extract importance 无上限，系统指令可得 0.9+ | P2 open |

### Tier 3 — 排序精度（决定 top-K 里好结果的位置）

| # | Issue | 问题 | 状态 |
|---|-------|------|------|
| 6 | **ISS-002** | Recency bias 不足 — ACT-R decay 让新旧记忆权重差异不够 | ✅ closed |
| 7 | **ISS-007** | 无 confidence score — recall 结果无法区分高相关与噪声 | P1 closed |
| 8 | **ISS-008** | Knowledge promotion — 高频记忆不自动提升到 SOUL.md/MEMORY.md | P1 open |

### 已解决

| Issue | 问题 | 修复 |
|-------|------|------|
| **ISS-010** | embedding model_id 格式不一致 — 丢失 58% 数据 | ✅ 2026-04-08 |
| **ISS-006** | 单一检索路径 | ✅ hybrid search 已实现 (FTS5 15% + Embedding 60% + ACT-R 25%) |

### 跨 Issue 能力：Supersedes（版本管理）

**问题**：同一事实存了多个版本（如"gid-rs 485 个测试" → "gid-rs 1071 个测试"），recall 不保证返回最新版本。旧的过时信息可能因 ACT-R activation 更高反而排在前面。

**方案**（已在 `docs/engram-hub-discussion.md` 讨论过）：给 Hebbian link 加 `supersedes` 关系类型 —— 检测到新版本知识时自动标记旧版本为被 supersede，recall 时降权或排除。

**这不是独立 issue**，而是 ISS-003 + ISS-005 + ISS-009 三个 issue 都解决后自然具备的能力：
1. **ISS-009 Entity 索引** — 知道两条记忆说的是同一件事（同 entity）
2. **ISS-003 Dedup on write / ISS-005 Consolidate** — 写入时检测或 consolidate 时合成，触发 supersede 标记
3. **ISS-007 Confidence** — Recall 时对 superseded 记忆降权

**参考**: `docs/engram-hub-discussion.md` §Typed Hebbian Links — `supersedes` 类型定义 + confidence 防线 + 自动触发机制（待定）

### 推进策略（2026-04-09 正式启动）

**Phase A — 止血（RustClaw 侧，不改 engramai crate）**

| Task | 描述 | 改动位置 | 规模 | 状态 |
|------|------|----------|------|------|
| **A1** | EngramStoreHook 过滤 — 不存 HEARTBEAT_OK / NO_REPLY / 系统指令 | `rustclaw/src/engram_hooks.rs` | ~30 行 | done |
| **A2** | Extractor prompt 加 negative examples — 不提取系统指令/agent身份 | `engramai/src/extractor.rs` | ~20 行 | done |
| **A3** | SQL 一次性清理 — 删精确重复(105条) + 系统指令垃圾 | SQL script | 一次性 | done |

过滤策略（A1 详细）：
- ❌ `HEARTBEAT_OK` → 直接丢，零信息
- ❌ `NO_REPLY` → 直接丢，零信息
- ❌ 系统指令特征（"你是 RustClaw"、"Read SOUL.md" 等）→ 直接丢
- ⚠️ Heartbeat 有实质内容的响应 → 保留，让 extractor 正常处理

**Phase B — 提质（改 engramai crate，走 ritual）**

| Task | 描述 | ISS | 规模 | 依赖 | 状态 |
|------|------|-----|------|------|------|
| **B1** | Entity 索引实现 — extraction on write + entity storage + entity-aware recall | ISS-009 | ~500-800 行 | A1,A2 | todo |
| **B2** | Dedup on write — embedding similarity check + merge 策略 | ISS-003 | ~200 行 | A3 | todo |
| **B3** | Confidence score — 多信号融合 recall confidence 0-1 | ISS-007 | ~100 行 | B1 | todo |
| **B4** | Recency 调参 — ACT-R decay parameter 分析 + 调整 | ISS-002 | ~20 行 + 分析 | B3 | ✅ done |

**依赖图**：
```
A1 (hook filter) ──┐
A2 (extractor)  ───┼── A3 (SQL cleanup) ──→ B1 (entity) ──→ B3 (confidence)
                   │                   └──→ B2 (dedup)       └──→ B4 (recency)
```

**GID 跟踪**: 所有任务在 RustClaw `.gid/graph.yml` 中管理（engram 项目无独立 graph）

---

## ISS-001 [bug] [P0] [closed]
**标题**: consolidate 命令 SQLite corruption
**发现日期**: 2026-03-29
**发现者**: RustClaw
**组件**: consolidate (SQLite UPDATE operations)
**跨项目引用**: —

**描述**:
`engram consolidate` 命令失败，报错 `database disk image is malformed`。UPDATE 操作触发 SQLite corruption。

**上下文**:
从 2026-03-29 首次发现，持续存在。FTS5 full-text search index 曾重建过一次，但 consolidate 的 UPDATE 操作仍然偶发失败。可能是 WAL mode 下的并发写入问题（RustClaw crate + CLI 同时写 DB），或者 FTS5 index 再次损坏。

**建议方案**:
- 检查是否有并发写入（RustClaw + CLI 同时写 DB）
- consolidate 前做 `PRAGMA integrity_check`
- 如果 FTS5 损坏，自动 rebuild：`INSERT INTO memories_fts(memories_fts) VALUES('rebuild')`
- 考虑 consolidate 用 exclusive lock

**相关**:
- `INVESTIGATION-2026-03-31.md` 有详细分析

---

## ISS-002 [improvement] [P1] [closed]
**标题**: Recall recency bias 不足 — ACT-R decay 参数需调整
**发现日期**: 2026-04-05
**发现者**: RustClaw
**组件**: recall (ACT-R activation scoring)
**跨项目引用**: —

**描述**:
Recall 的 recency bias 不足。ACT-R activation 的 decay parameter `d` 可能需要调整，让近期记忆权重更高。当前旧记忆和新记忆在 scoring 上差异不够明显。

**上下文**:
实际使用中，几天前的相关记忆经常排在刚刚存入的记忆后面。对于 agent 场景，recency 应该比学术 ACT-R 模型更重要。

**建议方案**:
- 检查当前 decay parameter d 的值
- 增大 d 值让 recency 权重更高
- 或者在 scoring 中加一个 recency bonus factor

**相关**:
- `LEARNINGS.md` "Recall Quality Improvements" 部分

---

## ISS-003 [improvement] [P2] [closed]
**标题**: add() 缺少 dedup 检查 — 重复记忆导致 DB 膨胀
**发现日期**: 2026-03-31
**更新日期**: 2026-04-09
**发现者**: RustClaw
**组件**: add() / memory extractor
**跨项目引用**: —

**描述**:
`add()` / `add_raw()` 没有任何 dedup 检查。相同或极其相似的内容可以重复存入，导致 DB 膨胀和 recall 结果重复。

**量化调查 (2026-04-09)**:
- 精确重复：105 条（content hash 完全相同）
- 近似重复：217 条（embedding cosine > 0.95）
- 语义集群过度集中：265 条（如 "potato 身份" 268 条同义表述、"heartbeat 指令" 150 条、任务引用 110 条）
- **实际重复率 ~5%**（非之前估计的 20-30%），短期可接受
- 但高频集群在搜索时挤占排名，影响 recall 质量
- **根因**：`add_raw()` 零 dedup 检查，直接写入

**关键设计决策 — 合并而非删除**:
Dedup 时必须**合并元数据**，不能简单删除重复条目。原因：
- ACT-R base-level activation 基于 access 历史。105 条重复各自有独立 access，激活值分散。合并后 1 条继承所有 access → activation 更准确
- Hebbian links 在重复记忆之间形成噪声链接（同义内容共现 ≠ 真正知识关联）。Dedup 后噪声消失 → hebbian 图谱更干净
- **结论：dedup 后 ACT-R 排序和 hebbian 质量都会改善，不会损害**

合并策略：
```
access_count = sum(所有重复的 access)
importance = max(所有重复的 importance)
created_at = min(最早的那条)
hebbian_links = 去重合并（指向非重复记忆的保留）
content = 保留最完整/最新的版本
```

**建议方案**:
- add() 前做 embedding similarity 检查，>0.95 的跳过或合并
- 或者用 content hash 做精确 dedup
- 更好的方案：借鉴 Mem0 的 Reconcile —— 新 fact 与已有记忆对比，决定 ADD / UPDATE / DELETE / NOOP
- extractor 端加 negative examples 避免提取系统指令
- **实现时可与 ISS-009 entity 建设一起做**（建实体时顺便去重）

**相关**:
- `MEMORY-SYSTEM-RESEARCH.md` §1.2 Mem0 Reconcile 阶段
- `INVESTIGATION-2026-03-31.md` 垃圾记忆分析
- ISS-009 (entity 索引) — 可一并实现

---

## ISS-004 [improvement] [P2] [open]
**标题**: 中文分词对 recall 质量的影响 — FTS5 tokenizer 支持有限
**发现日期**: 2026-04-05
**发现者**: RustClaw
**组件**: recall (中文支持)
**跨项目引用**: —

**描述**:
中文分词对 recall 质量有影响。搜"认知层"可能找不到包含"认知"的记忆，因为 FTS5 默认 tokenizer 对中文支持有限。

**上下文**:
Engram 的用户（potato + RustClaw）中英混用。embedding-based recall 缓解了这个问题，但 FTS5 的 keyword recall 部分仍有问题。

**建议方案**:
- 加 jieba 分词或 bigram indexing for FTS5
- 或者更依赖 embedding recall，降低 FTS5 权重
- hybrid scoring 已有 `score_alignment_hybrid()` 可以参考

**相关**:
- IDEA-20260405-01 (Engram 认知层协议)

---

## ISS-005 [missing] [P1] [closed]
**标题**: consolidate 缺少知识合成 — 只加强 activation 不合成 insight
**发现日期**: 2026-03-31
**发现者**: RustClaw
**组件**: consolidate
**跨项目引用**: —

**描述**:
当前 consolidate 只是"加强"记忆（增加 activation），而不是"合成新知识"。Hindsight 的 Observation Consolidation 能从多条 facts 合成高阶 insight，这是 engram 最大的功能缺口。

**上下文**:
见 MEMORY-SYSTEM-RESEARCH.md §1.1 Hindsight 分析。真正有价值的 consolidation 应该是：多条相关记忆 → 合成一条新的 insight 记忆，而不只是更新 activation score。

**建议方案**:
- LLM-based consolidation：定期扫描相关记忆簇，让 LLM 合成高阶 insight
- 新 insight 存储时标记 source memories
- 与 Hebbian links 结合：频繁共激活的记忆优先做 consolidation

**相关**:
- `MEMORY-SYSTEM-RESEARCH.md` §1.1 Hindsight
- `ENGRAM-V2-DESIGN.md` 情感总线设计

---

## ISS-006 [missing] [P1] [closed]
**标题**: ~~Recall 缺少多路检索~~ → Hybrid search 已实现
**发现日期**: 2026-03-31
**关闭日期**: 2026-04-09
**发现者**: RustClaw
**组件**: recall (多路检索)
**跨项目引用**: —

**描述**:
~~目前 recall 只有 FTS5 + ACT-R，缺少 embedding 向量搜索和 entity graph 检索。~~

**已解决**: Hybrid search 已在 engramai 中实现，三路融合：
- **FTS5** (15%) — 关键词匹配
- **Embedding vector search** (60%) — 语义相似度（cosine similarity）
- **ACT-R activation** (25%) — 记忆衰减 + 使用频率

截至 2026-04-09，7,058 / 7,972 memories 已有 embeddings（88.5%）。`recall()` 默认走 hybrid 路径，embedding 通道权重最高。

**剩余改进空间（不再是 blocker，降级为 nice-to-have）**:
- Entity graph 检索（第4路）— 见 ISS-009
- RRF (Reciprocal Rank Fusion) 替代当前 weighted sum — 可能改善排序质量
- 权重可配置化（目前硬编码 15/60/25）

**相关**:
- `MEMORY-SYSTEM-RESEARCH.md` §1.1 TEMPR 4路检索
- RustClaw 的 `score_alignment_hybrid()` 是起点

---

## ISS-007 [improvement] [P1] [closed]
**标题**: Recall 结果缺少 confidence score — 无法区分高相关与噪声
**发现日期**: 2026-04-05
**关闭日期**: 2026-04-09
**修复 commit**: `c03a339`
**发现者**: RustClaw
**组件**: recall (confidence scoring)
**跨项目引用**: rustclaw (auto-recall hook 需要 confidence 做过滤)

**描述**:
Recall 结果没有有意义的 confidence score。难以区分"高度相关匹配"和"模糊相关噪声"。

**上下文**:
下游消费者（RustClaw auto-recall hook）无法做有效过滤，导致低质量记忆被注入 system prompt 浪费 context window。

**建议方案**:
- 从多个信号计算 confidence：embedding similarity, ACT-R activation, recency, keyword overlap
- 归一化到 0.0-1.0
- 返回每个 recall 结果的 confidence，让消费者按阈值过滤

**相关**:
- `LEARNINGS.md` "Confidence Score Calculation" 部分

---

## ISS-008 [feature] [P1] [open]
**标题**: Knowledge promotion — 高频记忆自动提升到上层文档
**发现日期**: 2026-04-07
**发现者**: potato
**组件**: consolidate → knowledge promotion
**跨项目引用**: rustclaw (engram_soul_suggestions 工具是早期原型)

**描述**:
当某类记忆反复出现并被 consolidate 强化后，应该自动提取总结，提升（promote）到上层文档（如 SOUL.md、MEMORY.md、AGENTS.md 等）。当前 consolidate 只增加 activation，不会产生"这个 pattern 已经稳定到可以写进 SOUL"的判断。

**上下文**:
例如 potato 多次强调"root fix, not patch"、"不要简化问题"、"第一性原理"—— 这些分散在几十条 engram 记忆里，但直到 potato 手动要求，才被写进 SOUL.md。理想情况下，engram 应该能检测到"这个原则已经被重复提到 N 次，activation 超过阈值，应该 promote 到 SOUL/MEMORY"。

**设计思路**:
- **检测层**：consolidate 时扫描高 activation 记忆簇（同主题、高 Hebbian link density）
- **提取层**：对簇内记忆做 LLM 总结，生成一条 crystallized principle/fact
- **推荐层**：不自动改文件 — 生成 promotion suggestion（"建议把以下原则写入 SOUL.md：..."），等人类 approve
- **去重层**：检查目标文档是否已有类似内容，避免重复写入
- **阈值**：activation > X 且 cluster size > N 且 time span > T（不是一天内重复说的，而是跨天持续出现的）

**与现有 ISS 的关系**:
- ISS-005（consolidate 合成新知识）是基础 — 先能合成 insight，才能 promote
- ISS-003（dedup）保证不会因为重复存入而人为抬高 activation
- ISS-007（confidence scoring）提供 promotion 决策的信号之一

**建议方案**:
1. 在 consolidate 流程末尾加 promotion check（高 activation 簇 → 生成 suggestion）
2. suggestion 输出到 `engram suggestions` 命令（或 `engram soul-suggestions`）
3. Agent 在 heartbeat 时检查 suggestions，向用户汇报
4. 用户 approve 后，agent 写入目标文档 + 标记相关记忆为 "promoted"

**相关**:
- ISS-005 (consolidate 合成新知识)
- ISS-003 (dedup)
- RustClaw 的 `engram_soul_suggestions` 工具是这个思路的早期原型
- 今天 SOUL.md 加 Engineering Philosophy 就是手动版的 promotion

---

## ISS-009 [feature] [P1] [open]
**标题**: Entity 索引 — schema 已建但完全空置，需实现写入+检索
**发现日期**: 2026-04-08
**更新日期**: 2026-04-09
**发现者**: RustClaw + potato
**组件**: entities, memory_entities, entity_relations (schema), write path, recall path
**跨项目引用**: rustclaw (agent 侧 recall 策略), gidhub (触发场景涉及 gid infer)

**触发场景**:
potato 问"我们之前讨论过 gid infer 吗？" — RustClaw 搜 `"gid infer"` 找不到，但 gidhub requirements（含相关概念）实际存在于 engram 中。问题是 "infer" 和 "gidhub requirements" 在人脑中关联，但 engram 中是孤立的 embedding 向量。

**问题本质 (2026-04-09 调查更新)**:

Schema 完备但**零数据、零代码调用**：
- `entities` 表：0 行（应有项目名、概念、人名等）
- `memory_entities` 表：0 行（应有 memory↔entity 关联）
- `entity_relations` 表：0 行（应有 entity↔entity 关系）
- **代码中零处 `INSERT INTO entities`** — 这三张表是死代码
- 唯一的关联机制是 `hebbian_links`（34,859 行），但这是 memory↔memory 级别，无法做概念级跳转

**这是 recall 质量的天花板。** 当前 engram 是纯向量搜索 + 统计共现的系统，缺少知识图谱层。Entity 索引就是那个缺失的层。

**影响分析**:
- 搜 "infer" → 只能命中包含这个词的记忆
- 无法做 "infer" → gid-rs 功能 → gid-rs 还有哪些相关记忆？这种概念级跳转
- 7300+ 条记忆之间的关联全靠 embedding 相似度 + hebbian 共激活，没有结构化实体图谱

**需要实现的三层**:

### 1. Entity Extraction on Write（写入时提取）
- `add()` / `add_raw()` 写入记忆时，自动提取实体（项目名、概念、人名、工具名等）
- 写入 `entities` 表（去重 upsert）
- 写入 `memory_entities` 关联表
- 提取方式：LLM extraction 或 regex/NER 混合

### 2. Entity Relation Building（实体关系构建）
- 同一条记忆中共现的实体 → 自动建立 entity↔entity 关系
- 关系类型：has_feature, part_of, related_to, created_by 等
- 写入 `entity_relations` 表
- 可与 Hebbian 机制协同：高频共现的实体对加强关系权重

### 3. Entity-Aware Recall（实体感知检索）
- Recall 时先匹配 entity → 再找 entity 关联的记忆
- 多跳：query → entity → related entities → memories
- 作为第 4 路检索通道，融入现有 hybrid search（FTS5 15% + Embedding 60% + ACT-R 25% + Entity ?%）

**相关 issue**:
- ISS-003 (dedup) — 可一并实现：建实体时顺便去重
- ISS-005 (consolidate 知识合成) — entity 图谱为 consolidation 提供结构化输入
- ISS-007 (confidence scoring) — entity match 可作为 confidence 信号之一
- ISS-008 (knowledge promotion) — entity 高频出现可触发 promotion

---

## ISS-010 [bug] [P0] [closed]
**标题**: Recall 丢失 58% 数据 — embedding model_id 格式不一致
**发现日期**: 2026-04-08
**修复日期**: 2026-04-08
**发现者**: potato
**组件**: storage (embedding model_id format)
**跨项目引用**: —

**描述**:
生产数据库中 embedding 的 `model` 字段存在两种格式：
- **旧格式** (3,742 条): `nomic-embed-text` — v0.2.0 时用 `config.model` 直接写入
- **新格式** (2,710 条): `ollama/nomic-embed-text` — 后来改为 `config.embedding.model_id()` 返回带 provider 前缀

recall 查询按当前 model_id (`ollama/nomic-embed-text`) 过滤，导致 3,742 条旧数据全部被跳过，**58% 的 embedding 数据对 recall 不可见**。

**根因**:
1. v0.2.0 → v0.2.1 改了 model_id 生成逻辑（加了 provider 前缀），但未迁移已有数据
2. v1→v2 schema migration 中有修复逻辑，但生产 DB 已经是 v2，该迁移不会再执行

**修复方案（两步 root fix）**:

### 1. 数据修复
```sql
UPDATE memory_embeddings SET model = 'ollama/nomic-embed-text' WHERE model = 'nomic-embed-text';
-- 3,742 rows updated
```

### 2. 代码防御
在 storage 层新增 `normalize_model_id()` 函数，对 5 个关键函数（写入/读取/删除）的 model 参数自动规范化：
- 无论调用方传入 `nomic-embed-text` 还是 `ollama/nomic-embed-text`，都统一为带 provider 前缀的格式
- 防止未来任何代码路径再写入裸模型名

**验证**: 测试通过，生产数据已全部统一为 `ollama/nomic-embed-text`。

**相关**:
- ISS-009 (recall 质量低) — 本 bug 是 recall 丢数据的直接原因之一

---

## ISS-011 [improvement] [P2] [closed]
**标题**: Recall 结果去重 — top-K 结果含近似重复，浪费 context window
**发现日期**: 2026-03-31
**整理日期**: 2026-04-16
**发现者**: RustClaw
**组件**: recall (result post-processing)
**跨项目引用**: rustclaw (auto-recall hook 注入 system prompt)

**描述**:
`recall()` 返回的 top-K 结果中可能包含内容高度相似的多条记忆。写入时有 dedup（ISS-003），但 recall 返回时没有结果级去重。近似重复的结果浪费 agent 的 context window。

**来源**:
- `INVESTIGATION-2026-03-31.md` §4.2 Fix 6
- `LEARNINGS.md` "Recall Result Dedup"

**建议方案**:
- `recall()` 返回后加 post-processing：两条结果 content 相似度 > 0.8 → 只保留 activation/confidence 更高的
- 可用 embedding cosine 或简单的 token overlap 判断
- 去重后补位：如果去掉 2 条重复，从候选池补 2 条进来保持 K 条

**规模**: ~50-80 行

**相关**:
- ISS-003 (写入时 dedup) — 互补关系：写入去重减少存量，recall 去重清理输出

---

## ISS-012 [improvement] [P2] [closed]
**标题**: Auto-extract importance 缺校准上限 — 系统指令可获高 importance
**发现日期**: 2026-03-31
**整理日期**: 2026-04-16
**发现者**: RustClaw
**组件**: extractor (importance scoring)
**跨项目引用**: —

**描述**:
Haiku extractor 提取的 importance 没有上限/基线校准。理论上系统指令也能得到 importance 0.9+（虽然 A2 的 negative examples 缓解了部分问题，但对漏网之鱼仍无校准）。

**来源**:
- `INVESTIGATION-2026-03-31.md` §4.2 Fix 5

**建议方案**:
- Auto-extracted facts 的 importance 上限 0.7（不能高于手动存储的基线）
- `source: "auto-extract"` 的记忆 importance cap 在 0.7
- `source: "manual"` / `source: "agent"` 的不受限
- Procedural memory 默认 importance 0.5（除非内容明显是用户偏好/工作流）

**规模**: ~20-30 行

**相关**:
- Phase A2 (extractor negative examples) — A2 防提取，本 issue 防过高评分

---

## ISS-013 [maintenance] [P3] [open]
**标题**: STDP 自动因果链接质量审计 — 34,859 条 Hebbian link 未验证
**发现日期**: 2026-03-31
**整理日期**: 2026-04-16
**发现者**: RustClaw
**组件**: consolidate (Hebbian / STDP)
**跨项目引用**: —

**描述**:
`hebbian_links` 表有 34,859 行，其中 `stdp:auto` 标记的约 115 条是 STDP (Spike-Timing Dependent Plasticity) 自动创建的因果链接。这些是 consolidation 时两条记忆频繁被一起召回时自动建立的，但质量未经验证。

可能的问题：
- 因垃圾记忆（已由 A1/A2/A3 清理）产生的虚假链接仍在
- session 内高频共现 ≠ 真正因果关系
- 链接权重分布可能有偏

**来源**:
- `INVESTIGATION-2026-03-31.md` §3 STDP 分析
- `LEARNINGS.md` "STDP auto 115 条需要验证"

**建议方案**:
- 导出所有 stdp:auto 链接，人工抽样检查质量
- 删除两端任一记忆已被 A3 清理掉的僵尸链接
- 考虑加 decay：长期不被共同召回的 STDP 链接自动弱化

**规模**: 分析为主，代码改动 ~30-50 行

---

---

# Features: 认知神经科学组件

> 格式: FEAT-{NNN} [{priority}] [{status}]
> 来源: `COGNITIVE-COMPONENTS-BACKLOG.md`, `INTEROCEPTIVE-LAYER.md`, `ENGRAM-V2-DESIGN.md`, `MEMORY-SYSTEM-RESEARCH.md`
> 
> 每个 Feature 对应一组相关的认知科学组件。Feature 下的 Phase 是实现阶段。
> COGNITIVE-COMPONENTS-BACKLOG.md 保留完整理论背景，这里只跟踪实现状态。

---

## 总览

| Feature | 标题 | 组件数 | 设计文档 | 优先级 | 状态 |
|---------|------|--------|---------|--------|------|
| **FEAT-001** | 内感受层（脑岛） | 7 组件, 5 Phase | `INTEROCEPTIVE-LAYER.md` | P1 | todo |
| **FEAT-002** | 情感闭环 | 6 组件 | `ENGRAM-V2-DESIGN.md` | P2 | todo |
| **FEAT-003** | 记忆生命周期 | 8 组件 | `MEMORY-SYSTEM-RESEARCH.md` | P1 | partial |
| **FEAT-004** | 认知理论扩展 | 8 组件 | cognitive-autoresearch Doc 02 | P3 | todo |

---

## FEAT-001 [P1] [todo]
**标题**: 内感受层 — InteroceptiveHub（脑岛功能等价物）
**设计文档**: `INTEROCEPTIVE-LAYER.md`（完整 5-Phase 设计 + 架构图 + Rust 类型定义）
**Backlog 编号**: A1-A7
**总规模**: ~700-850 行（engram ~500-700, RustClaw ~150）

**核心论点**: engram 有 9 个认知模块各自运行良好，但互不知道对方存在。InteroceptiveHub 是把它们串成闭环的整合层——Craig 脑岛的计算等价物。

### Phase 分解

| Phase | 内容 | Backlog | 规模 | 依赖 | 状态 |
|-------|------|---------|------|------|------|
| **F1-P1** | 统一信号格式 `InteroceptiveSignal` | A1 | ~200 行 | 无 | todo |
| **F1-P2** | InteroceptiveHub 核心（信号接收 + 状态聚合 + somatic cache） | A2, A4, A6 | ~400 行 | F1-P1 | todo |
| **F1-P3** | GWT 全局广播（SessionWM 变化 → 广播到所有模块） | A3 | ~150 行 | F1-P2 | todo |
| **F1-P4** | 调节输出层（RegulationAction → SOUL 建议 / 检索策略调整） | A5 | ~200 行 | F1-P2 | todo |
| **F1-P5** | 前脑岛元表征（"我知道我现在的状态"自我模型） | A7 | ~150 行 | F1-P2, F1-P4 | todo |

### 依赖图
```
F1-P1 (信号格式) → F1-P2 (Hub 核心) → F1-P3 (GWT 广播)
                                      → F1-P4 (调节输出) → F1-P5 (元表征)
```

### 涉及文件
- **新增**: `engram/src/interoception.rs`（Hub 核心）
- **改动**: `anomaly.rs`, `bus/accumulator.rs`, `bus/feedback.rs`, `confidence.rs`, `bus/alignment.rs`（加 InteroceptiveSignal 输出）
- **改动**: `session_wm.rs`（加广播机制）
- **RustClaw 侧**: `memory.rs`（持有 Hub）, `engram_hooks.rs`（注入状态）

---

## FEAT-002 [P2] [todo]
**标题**: 情感闭环 — 从被动记录到主动循环
**设计文档**: `ENGRAM-V2-DESIGN.md`
**Backlog 编号**: B1-B6

**核心论点**: 当前 engram 的情感模块是被动记录器。闭环意味着：情绪影响记忆 → 记忆影响行为 → 行为产生新情绪。

### 组件分解

| # | 组件 | 描述 | 依赖 | 复杂度 | 状态 |
|---|------|------|------|--------|------|
| **F2-B1** | Emotion → SOUL 闭环 | 持续负面情绪趋势 → 自动生成 SOUL.md 更新建议 | FEAT-001 F1-P4 | 中 | todo |
| **F2-B2** | SOUL → Importance 自动调权 | SOUL drives 变化 → 相关记忆 importance 批量调整 | 无 | 低 | todo |
| **F2-B3** | Engram → HEARTBEAT 自适应 | 记忆异常多 → 增加巡检频率；稳定 → 降低 | FEAT-001 F1-P2 | 低 | todo |
| **F2-B4** | HEARTBEAT → 经验回流 | heartbeat 发现 → 作为经验存入 engram（带 source 标记） | 无 | 低 | todo |
| **F2-B5** | IDENTITY 自动演化 | 基于积累的经验和情感趋势，自动建议 IDENTITY.md 更新 | F2-B1 | 中 | todo |
| **F2-B6** | Voice 情感分析 | 语音特征（语速、音调、能量）→ 情绪标签 | 无 | 中 | todo |

### 关键依赖
- F2-B1 和 F2-B3 依赖 FEAT-001（InteroceptiveHub 提供整合后的状态信号）
- F2-B2 和 F2-B4 可独立实现

---

## FEAT-003 [P1] [partial]
**标题**: 记忆生命周期 — 从"存了就存了"到"记忆会演化"
**设计文档**: `MEMORY-SYSTEM-RESEARCH.md`
**Backlog 编号**: C1-C9

**核心论点**: 记忆不是存进去就结束。需要去重、合并、合成高阶知识、结构化为实体图谱。

### 组件分解

| # | 组件 | 描述 | ISS 关联 | 状态 |
|---|------|------|----------|------|
| **F3-C1** | Gate 入口过滤 | 输入→决定是否值得记忆 | Phase A1/A2 | ✅ done |
| **F3-C2** | Mission-Steered Extraction | SOUL.md 驱动引导提取方向 | — | todo |
| **F3-C3** | Embedding Reconciler | 新记忆 vs 已有 → embedding 距离去重/合并 | ISS-003 扩展 | todo |
| **F3-C4** | LLM Reconciler | 矛盾记忆 → LLM 决定保留/合并 | ISS-005 扩展 | todo |
| **F3-C5** | Observation Consolidation | 多条观察 → 合成一条知识 | ISS-005 | todo |
| **F3-C6** | Entity Graph | 实体关系网络 | **ISS-009** | todo |
| **F3-C7** | Multi-Retrieval Fusion | TEMPR 级多路融合检索 | ISS-006 扩展 | ✅ done |
| **F3-C8** | Working Context State Machine | 对话上下文状态管理 | — | todo |
| **F3-C9** | Activation Heatmap | 记忆激活分布可视化 | — | todo |

### 推荐顺序
1. **F3-C6** (ISS-009 Entity Graph) — recall 天花板，最高优先
2. **F3-C3** (Embedding Reconciler) — 用户最可感知的改善
3. **F3-C5** (Observation Consolidation) — ISS-005 的完整实现
4. **F3-C2** (Mission-Steered) → **F3-C4** (LLM Reconciler) → **F3-C7** (Multi-Retrieval)
5. **F3-C8**, **F3-C9** — nice-to-have

---

## FEAT-004 [P3] [todo]
**标题**: 认知理论扩展 — 从文献到代码
**设计文档**: cognitive-autoresearch Doc 02
**Backlog 编号**: D1-D8

**核心论点**: 认知科学有大量成熟理论可以为 engram 的记忆管理提供更好的模型。这些是研究性质的实现，优先级最低但长期价值最高。

### 组件分解

| # | 组件 | 认知理论 | 复杂度 | 状态 |
|---|------|---------|--------|------|
| **F4-D1** | STDP 因果方向 | Spike-Timing-Dependent Plasticity | 中 | todo |
| **F4-D2** | 神经调质全局调控 | Neuromodulation (DA/5-HT/ACh/NE) | 高 | todo |
| **F4-D3** | 感觉门控/丘脑过滤 | Sensory Gating / Thalamic Filter | 中 | todo |
| **F4-D4** | 稀疏编码 | Sparse Distributed Representation | 中 | todo |
| **F4-D5** | 神经振荡 | Neural Oscillations (Theta/Gamma) | 高 | todo |
| **F4-D6** | 小世界网络 | Small-World Network Topology | 高 | todo |
| **F4-D7** | 皮层柱 | Cortical Column / Minicolumn | 高 | todo |
| **F4-D8** | 时间细胞 | Time Cells (Hippocampal) | 中 | todo |

### 推荐顺序（按改动量/收益比）
1. **F4-D1** STDP — 改动最小（Hebbian 已有，加方向），收益最大（因果推理）
2. **F4-D8** 时间细胞 — 独立模块，改善时间相关检索
3. **F4-D3** 感觉门控 — 输入预处理层，与 FEAT-001 互补
4. **F4-D4** 稀疏编码 — top-k 激活机制
5. **F4-D2/D5/D6/D7** — 需要深度设计后再动，暂不排期

### 交叉区组件（最高优先级 — 两个来源都认为需要）
以下组件在 cognitive-autoresearch Doc 02 和 engram 内部设计文档中都被提及，已归入对应 Feature：
- **E1 GWT 全局广播** → FEAT-001 F1-P3
- **E2 Somatic Marker** → FEAT-001 F1-P2
- **E3 元认知 Control** → FEAT-001 F1-P5
- **E4 神经调质** → FEAT-004 F4-D2

---

## ISS-014 [improvement] [P2] [open]
**标题**: Storage Trait 抽象 — 解耦算法层与 SQLite 后端
**发现日期**: 2026-04-16
**发现者**: potato
**组件**: storage.rs
**跨项目引用**: —

**描述**:
当前 `storage.rs` (~2545 行) 直接调用 `rusqlite`，算法层（ACT-R、Hebbian、Infomap）与存储后端紧耦合。需要抽取 `MemoryStore` trait，让算法代码只依赖接口，不依赖具体存储实现。

**动机**:
- 多 agent 共享记忆是未来方向 — SQLite 单写者模型在 10+ agent 并发写入时扛不住
- 算法代码（ACT-R decay、Hebbian learning、Infomap clustering）只需要"给我节点"和"给我边"，不应关心数据存在哪
- 当前无法换后端而不重写 2500+ 行代码

**设计草案**:
```rust
trait MemoryStore {
    fn store(&self, memory: &Memory) -> Result<()>;
    fn recall(&self, query: &str, limit: usize) -> Result<Vec<Memory>>;
    fn update_hebbian(&self, a: &str, b: &str, delta: f64) -> Result<()>;
    fn get_hebbian_edges(&self) -> Result<Vec<(String, String, f64)>>;
    fn search_hybrid(&self, query: &str, embedding: &[f32]) -> Result<Vec<Memory>>;
    fn prune_weak(&self, threshold: f64) -> Result<usize>;
    // ... full interface TBD during implementation
}

struct SqliteStore { ... }     // 现有代码包装，零行为变更
// Future:
// struct PgStore { ... }       // PostgreSQL + pgvector
// struct SurrealStore { ... }  // SurrealDB
```

**范围**:
- 从 `storage.rs` 公开 API 提取 trait 接口
- 实现 `SqliteStore`（搬现有代码，零行为变更）
- 所有调用方改为依赖 trait 而非具体类型
- 所有现有测试必须通过，无修改

**优先级**: P2 — 当前不阻塞任何工作。在多 agent 记忆共享之前完成即可。

**上下文**:
2026-04-16 讨论：potato 问 SQLite 能否支持未来多 agent 并发写入。结论：SQLite 现阶段够用（2-3 agent），但算法层应与存储解耦，以便未来迁移无痛。

**相关**:
- ISS-009 (Infomap 集成) — 聚类算法不关心存储，但需要通过 trait 获取边列表
- Knowledge Compiler 产品 — 首个大规模记忆消费者
- 多 agent engram 共享 — 触发实际换后端的时机

---

# 全局实现路线建议 (2026-04-16)

**当前 open 的操作性 Issues（可直接做）：**
- ISS-009 (P1) Entity 索引 → 也是 FEAT-003 F3-C6
- ISS-008 (P1) Knowledge promotion
- ISS-015 (P1) 聚类算法升级 — Union-Find → Infomap（两步：KC统一聚类 → 算法替换）
- ISS-004 (P2) 中文分词
- ISS-011 (P2) Recall 结果去重
- ISS-012 (P2) Importance 校准
- ISS-013 (P3) STDP 审计
- ISS-014 (P2) Storage Trait 抽象 — 解耦算法层与存储后端

**Feature 推荐顺序：**
1. **ISS-009 + ISS-011 + ISS-012** — 操作性修复，直接提升 recall 质量
2. **FEAT-001 Phase 1-2** — 内感受层核心（信号格式 + Hub），解锁所有后续
3. **FEAT-003 F3-C3/C5** — Reconciler + Consolidation，记忆质量闭环
4. **FEAT-001 Phase 3-5** — GWT 广播 + 调节输出 + 元表征
5. **FEAT-002** — 情感闭环（大部分依赖 FEAT-001）
6. **FEAT-004** — 认知理论扩展（研究性质，按需推进）
