# Requirements: Knowledge Compiler

**Status**: P0 Complete — `src/compiler/` is live (discovery / compilation / conflict / decay / health / promotion). ISS-017 (O(n²)→HNSW) closed 2026-04-20. P1/P2 roadmap pending re-evaluation.
**Last reviewed**: 2026-04-20

## Project Structure Decision (2026-04-17)

**Decision: KC P0 代码在 engram-ai-rust 项目内开发，不单独立项。P1/P2 再评估。**

**理由：**
1. GUARD-1 明确要求 "所有 KC 功能必须作为 engram crate 的模块实现"
2. P0 的所有代码都写在 `src/compiler/`，测试也在 engram-ai-rust 里跑
3. KC task 跟 engram 现有模块（synthesis、entities、consolidation）高度耦合，放同一个 graph 可以直接表达 `depends_on` 关系
4. KC 的 requirements/design 文档已经在 `.gid/features/knowledge-compiler/` 下，结构清晰

**但 KC ≠ engram 的子集：**
- KC 会阉割 engram 的部分功能（feature flag）
- P1/P2 会加 engram 没有的产品层（Web UI、Cloud DB、用户系统、定价 tier）
- 到 P1/P2 阶段，可能需要创建独立项目目录（`/Users/potato/clawd/projects/knowledge-compiler/`），把产品层代码分离出去
- 核心引擎代码（`src/compiler/`）始终留在 engram crate 内

**Graph 策略：**
- KC 作为 engram-ai-rust graph 的顶层 component 节点
- 下挂 compilation / maintenance / platform 三个 feature
- 每个 feature 拆具体 task
- KC task 可以 `depends_on` engram 现有节点（如 synthesis、entities）
- P1/P2 产品层 task 到时候视情况建独立 graph 或继续在这里

---

## Overview

Knowledge Compiler 是 engram 的知识编译层：把碎片记忆（raw interactions、ideas、bookmarks、notes）编译成结构化、可浏览、会衰减的知识库。

**用户问题**：信息 intake 容易，但碎片记忆永远是碎片。Karpathy 用 LLM 手动编译成 wiki 页面，但没有衰减、没有强化、没有冲突检测、没有质量管理。Knowledge Compiler 自动做这些事。

**核心定位**：engram 已有 synthesis engine（碎片 → insight），Knowledge Compiler 在此之上加两层：
1. **Compilation 层** — insights + 记忆 → 可浏览的知识页面（topic pages）
2. **Maintenance 层** — 衰减、冲突检测、质量管理、链接修复

**商业模型：开源本地 + 云端收费**
- **开源（本地）**: engram crate + CLI + Knowledge Compiler 核心 + Markdown 导出，功能完整不阉割。类似 Obsidian 本地版但开源。
- **收费（云端）**: Cloud-hosted DB、Web 知识图谱浏览器、多设备同步、团队协作、更大 LLM 编译配额。SaaS 月费。
- **对标**: Obsidian（闭源本地免费 + Sync/Publish 收费）→ KC 做得更好：开源本地 + 云端收费卖便利性和协作。开源本地版是最好的获客渠道。

**和现有能力的关系**：
- engram synthesis engine（cluster → gate → insight）= 已有，是 KC 的输入
- engram consolidation（ACT-R 衰减、层级转移）= 已有，KC 复用
- engram dedup（存入时 + 召回时）= 已有，KC 复用
- Idea Intake Pipeline（RustClaw skill）= 已有，是 KC 的上游 intake
- **KC 新增的** = topic page 编译、page-level 衰减、跨页冲突检测、知识质量管理、导出

## Priority Levels

- **P0**: Core — 没有这个 KC 不成立
- **P1**: Important — 生产质量需要
- **P2**: Enhancement — 提升体验但不阻塞 MVP

## Guard Severity

- **hard**: 违反 = 系统坏了
- **soft**: 违反 = 质量下降，可容忍

---

## GUARDs — System-Wide Constraints

### GUARD-1: Engram-Native [hard]
所有 KC 功能必须作为 engram crate 的模块实现（或 engram CLI 子命令），不得创建独立 binary 或独立数据库。KC 的存储层复用 engram 的 SQLite + embedding 基础设施。

### GUARD-2: Incremental, Not Batch [hard]
编译过程必须是增量的：新记忆进来时只编译受影响的 topic pages，不重新编译整个知识库。全量重编译只作为手动 repair 操作存在。

### GUARD-3: LLM Cost Awareness [soft]
每次编译操作必须估算 LLM token 成本并记录。单次自动编译（非用户手动触发）的成本上限可配置（默认 $0.10）。超出时 defer 而非执行。

### GUARD-4: Provenance Traceability [hard]
每个 topic page 必须追溯到源记忆。用户能从 page 点回原始记忆，也能从记忆看到它贡献了哪些 pages。复用 synthesis provenance 机制。

### GUARD-5: Non-Destructive [hard]
编译过程不得删除或修改源记忆内容。源记忆的 importance/strength 可以调整（consolidation 已有机制），但 content 字段不可变。更广义地，任何自动化操作（安装、导入、文件处理）中途失败时，不得丢失已有数据或用户文件。

### GUARD-6: Offline-First [soft]
知识库的浏览和搜索操作本身不调用 LLM（query-time 无 LLM 依赖）。搜索结果可包含 LLM 预编译的 topic page 内容。LLM 只在编译和冲突检测时需要。导出的知识库是纯 Markdown，任何编辑器可打开。

---

## Feature Index

GOALs are split into feature-level requirement documents. Each feature doc references this master for GUARDs.

| Feature Doc | GOALs | Description |
|---|---|---|
| [`compilation/requirements.md`](compilation/requirements.md) | 10 GOALs (GOAL-comp.1–10) | Topic page generation, discovery, incremental compilation, merging/splitting, cross-topic linking, compilation failure handling, user feedback & control |
| [`maintenance/requirements.md`](maintenance/requirements.md) | 13 GOALs (GOAL-maint.1–5b, 6–12) | Page-level decay, conflict detection, broken link repair, duplicate detection, health reports, operation summaries, knowledge-aware recall, export, CLI, programmatic API, privacy & data security |
| [`platform/requirements.md`](platform/requirements.md) | 16 GOALs (GOAL-plat.1–16) | Multi-provider LLM support, configuration, graceful degradation, setup & embedding strategy, data import, import progress & error reporting, config migration, intake channels |

### Cross-Reference: Old GOAL → New GOAL

| Old ID | New ID | Feature Doc |
|---|---|---|
| GOAL-kc.1 | GOAL-comp.1 | compilation |
| GOAL-kc.2 | GOAL-comp.2 | compilation |
| GOAL-kc.3 | GOAL-comp.3 | compilation |
| GOAL-kc.4 | GOAL-comp.4 | compilation |
| GOAL-kc.5 | GOAL-comp.5 | compilation |
| GOAL-kc.6 | GOAL-comp.6 | compilation |
| GOAL-kc.23 | GOAL-comp.7 | compilation |
| GOAL-kc.24 | GOAL-comp.8 | compilation |
| GOAL-kc.25 | GOAL-comp.9 | compilation |
| GOAL-kc.7 | GOAL-maint.1 | maintenance |
| GOAL-kc.8 | GOAL-maint.2 | maintenance |
| GOAL-kc.9 | GOAL-maint.3 | maintenance |
| GOAL-kc.10 | GOAL-maint.4 | maintenance |
| GOAL-kc.11 | GOAL-maint.5 | maintenance |
| GOAL-kc.12 | GOAL-maint.6 | maintenance |
| GOAL-kc.13 | GOAL-maint.7 | maintenance |
| GOAL-kc.14 | GOAL-maint.8 | maintenance |
| GOAL-kc.15 | GOAL-maint.9 | maintenance |
| GOAL-kc.26 | GOAL-maint.10 | maintenance |
| GOAL-kc.27 | GOAL-maint.11 | maintenance |
| GOAL-kc.28 | GOAL-maint.12 | maintenance |
| GOAL-kc.16 | GOAL-plat.1 | platform |
| GOAL-kc.17 | GOAL-plat.2 | platform |
| GOAL-kc.18 | GOAL-plat.3 | platform |
| GOAL-kc.33 | GOAL-plat.4 | platform |
| GOAL-kc.34 | GOAL-plat.5 | platform |
| GOAL-kc.35 | GOAL-plat.6 | platform |
| GOAL-kc.36 | GOAL-plat.7 | platform |
| GOAL-kc.19 | GOAL-plat.8 | platform |
| GOAL-kc.20 | GOAL-plat.9 | platform |
| GOAL-kc.21 | GOAL-plat.10 | platform |
| GOAL-kc.22 | GOAL-plat.11 | platform |
| GOAL-kc.29 | GOAL-plat.12 | platform |
| GOAL-kc.30 | GOAL-plat.13 | platform |
| GOAL-kc.31 | GOAL-plat.14 | platform |
| — (new) | GOAL-comp.10 | compilation |
| — (new) | GOAL-maint.5b | maintenance |
| — (new) | GOAL-plat.15 | platform |
| — (new) | GOAL-plat.16 | platform |

---

## §4 — Distribution（分发形式 + 可视化策略）

### 开源 vs 收费分层

```
┌─────────────────────────────────────────────────────┐
│  开源（本地，MIT/Apache）                            │
│                                                     │
│  engram crate          — 引擎（compile/decay/detect）│
│  engram CLI            — compile/topics/export       │
│  Idea Intake Skill     — RustClaw 上游 intake        │
│  Markdown 导出          — Obsidian 兼容 wikilinks    │
│  Knowledge Compiler 核心 — 全部 P0 功能，不阉割       │
│                                                     │
│  → cargo install / brew install，本地跑，完全免费     │
└─────────────────────────────────────────────────────┘
                         │
                    获客漏斗
                         │
                         ▼
┌─────────────────────────────────────────────────────┐
│  收费（云端，SaaS 月费）                              │
│                                                     │
│  Cloud-hosted engram DB — 免部署免维护                │
│  Web 知识图谱浏览器     — 全景可视化 + 交互           │
│  多设备同步             — 手机/电脑/平板               │
│  团队协作               — 共享知识库 + 权限管理        │
│  LLM 编译配额           — 本地要自己付 API 费，       │
│                          云端包含在订阅里              │
│  API access             — 第三方 agent/app 调用       │
│                                                     │
│  → Web 注册，月费按用量分 tier                        │
└─────────────────────────────────────────────────────┘
```

### 产品路线图

```
Phase 1 (P0 — 自用验证):
  开源本地版全部功能
  RustClaw Telegram intake + query + auto-compile
  CLI 手动操作
  Obsidian Markdown 导出验证知识图谱价值

Phase 2 (P1 — 产品化):
  自建 Web 知识图谱浏览器
  Cloud-hosted engram DB
  Web intake（浏览器扔内容）
  用户账户 + 定价 tier

Phase 3 (P2 — 规模化):
  团队协作 + 共享知识库
  多设备同步
  第三方 API
  Marketplace（知识模板/公共知识库）

Future (待详细设计):
  KC 自动生成 Issue 文档 — 聚类编译的 insight 如果是
  "未解决的技术问题/改进方向"，自动生成 .gid/issues/ 草稿
  → 通知人确认。需要：意图分类、issue 模板映射、去重检查。
  (2026-04-18 potato 提出)
```

### 形式 1: engram crate 内置模块 [P0]
- 所有 KC 功能作为 `engramai` crate 的 `compiler` 模块
- 路径：`src/compiler/` (mod.rs, topic.rs, maintenance.rs, export.rs)
- 用户 `cargo add engramai` 即获得 KC 能力
- 通过 `MemoryConfig` 的 `compiler` 字段启用/配置

### 形式 2: engram CLI 子命令 [P0]
- `engram compile / topics / topic / kb-health / kb-export`
- 个人用户的主要交互方式
- 集成进 RustClaw heartbeat（定期 `sleep_cycle` 已有，加 `compile` 步骤）

### 形式 3: RustClaw / Agent Integration [P0]
- **Intake**: Agent 的 idea intake pipeline 照常工作，记忆进 engram DB，无需用户改变习惯
- **Query**: Agent recall 时自动获得 topic page 增强（GOAL-maint.6），回答质量更高、token 更省
- **Compile**: Agent 的 sleep cycle（heartbeat）自动运行编译 + 维护，用户无感
- **Browse**: Telegram bot commands (`/topics`, `/topic <id>`, `/kb-health`) 做轻量浏览
- **这是用户的主要 intake + query 渠道**，但不适合全景浏览（线性消息流限制）

### 形式 4: Obsidian Markdown 导出 [P1]
- `engram kb-export ./vault --format obsidian`
- 输出带 `[[wikilinks]]` 的 Markdown，每个 topic page 一个 .md 文件
- Obsidian Graph View 浏览知识图谱关系
- 支持增量导出（只导出变化的 pages，基于 stale 标记）
- **定位**: Phase 1 的可视化方案，验证知识图谱浏览是否有价值
- **单向导出，不做双向同步** — Obsidian 插件开发成本高且最终会被自建 UI 替代

### 形式 5: 自建 Web 知识图谱浏览器 [P2]
- **Phase 2 的可视化方案** — 验证 Obsidian 导出确实有用后再建
- 复用 RustClaw dashboard（localhost:8081）或独立端口
- Graph 可视化引擎（D3.js / Cytoscape.js）
- 核心交互：
  - 节点 = topic page，大小 = 活跃度，颜色 = 领域/domain
  - 边 = cross-topic links，粗细 = Hebbian 强度 / 共享记忆数
  - 点击节点 → 展开 topic page 内容 + 源记忆列表
  - 冲突高亮（红色边/节点）、stale 灰化、archived 虚线
  - 搜索 + 过滤（按领域、活跃度、时间范围）
- **Obsidian 做不到的**：
  - 节点大小/颜色按 engram 活跃度动态映射
  - 衰减曲线可视化
  - 冲突标记直接在图上显示
  - 不需要用户装桌面软件
- **这是 SaaS 产品形态的前端** — 当前阶段不实现，但 API 设计要为此预留
- GOAL-maint.9 的 Rust API 必须是 Web UI 可直接调用的粒度

### 可视化决策记录

**为什么不做 Obsidian 插件？**
- 双向同步开发成本高（TypeScript, Obsidian API, 冲突解决逻辑）
- Obsidian API 变动需要持续跟进维护
- 最终目标是自建 Web UI — Obsidian 插件投入会被废弃
- 纯导出已经满足 Phase 1 的验证需求

**为什么最终要自建而非依赖 Obsidian？**
- Obsidian Graph View 不可定制（节点大小/颜色不能映射到活跃度）
- 无法显示 engram 特有信息（衰减、Hebbian 强度、冲突）
- SaaS 产品不能要求用户装桌面软件
- Knowledge Compiler 的差异化价值（衰减、强化、冲突检测）需要在可视化层体现

**Phase 1 → Phase 2 的触发条件**：
- 用户（potato 自己）在 Obsidian 里实际浏览知识库 ≥2 周
- 确认 topic page 质量够用，编译逻辑稳定
- 明确感受到 Obsidian 的限制（不能按活跃度排序/过滤等）

---

## Out of Scope (当前版本不做)

- **Multi-user / 团队知识库** — 当前只支持单 engram DB = 单用户
- **多设备同步（本地版）** — SQLite 单文件不支持并发写。Phase 1 明确为单设备使用。用户如需多设备，用云端付费版。本地文件通过 Dropbox/iCloud 同步 SQLite 会导致写冲突损坏数据库，不推荐也不支持。
- **Real-time collaboration** — 不是 Google Docs
- **Obsidian 双向同步插件** — 只做单向导出。开发成本高且最终被自建 UI 替代
- **Web UI 实现** — Phase 2，API 预留但 Phase 1 不实现
- **Knowledge marketplace** — 这是 Engram Hub (IDEA-20260406-03) 的范围
- **Image/video 内容理解** — 只处理文本记忆
- **Custom LLM fine-tuning** — 用通用 LLM + prompt engineering

---

## Dependencies

- **engram synthesis engine** — cluster discovery, gate check, insight generation, provenance
- **engram consolidation** — ACT-R activation, Ebbinghaus decay, layer transitions
- **engram dedup** — embedding-based dedup at store and recall time
- **engram entities** — entity extraction for cross-topic linking
- **engram hybrid_search** — FTS5 + embedding search for topic recall
- **LLM provider** — for topic page compilation and conflict detection（复用 synthesis LLM provider）
