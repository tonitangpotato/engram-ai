# Requirements: Knowledge Maintenance, Access & Privacy

This feature covers the knowledge maintenance lifecycle (decay, conflict detection, broken link repair, duplicate detection, health reporting), access patterns (knowledge-aware recall, Markdown export, CLI subcommands, programmatic API), and privacy/data security guarantees for the local-first architecture. See [master requirements.md](../requirements.md) for GUARDs (system-wide constraints GUARD-1 through GUARD-6).

> **Note**: Section numbers (§2, §3, §8) follow master requirements numbering. Sections §1, §4–§7 are in other feature documents.

### Dependencies

- GOAL-maint.6, maint.7, maint.8 depend on compilation feature (GOAL-comp.1–5) producing topic pages.
- GOAL-maint.9 mirrors GOAL-maint.8 operations via programmatic API.

---

## GOALs — Functional Requirements

### §2 — Knowledge Maintenance（维护层）

#### GOAL-maint.1: Page-Level Decay [P0]
Topic pages 有活跃度评分，基于其源记忆的 ACT-R 激活度加权平均。长期未被访问/引用的 pages 活跃度下降。低活跃度 pages 在浏览时排序靠后，活跃度评分低于可配置阈值（默认值由 design 定义）时标记为 archived。

**Pass/fail**: 一个 topic page 的所有源记忆 30 天未被 recall，该 page 的活跃度评分下降 >50%。一个 page 被频繁 recall 相关记忆时活跃度上升。活跃度低于阈值的 page 自动标记为 archived。

#### GOAL-maint.2: Conflict Detection [P0]
当新记忆与已有 topic page 的内容矛盾时，系统检测并标记冲突。冲突检测使用 synthesis engine 的 Contradiction insight type + 语义比对。

**Pass/fail**: 添加一条与现有 topic page 某要点直接矛盾的记忆，系统在下一次编译周期检测到冲突并标记，标记包含冲突的具体要点和新记忆 ID。

**Degradation**: LLM 不可用时，conflict detection 降级为 embedding-only 比对（精度降低但不阻塞维护流程），并在 health report 中标记 "degraded mode: LLM unavailable"。

#### GOAL-maint.3: Broken Link Repair [P1]
当 topic page 引用的源记忆被删除或 archived 时，系统检测 broken references 并尝试修复（找替代记忆）或标记为 broken。维护操作作为 compile 周期的一部分自动运行，也可通过 CLI 手动触发（具体命令由 GOAL-maint.8 定义）。

**Pass/fail**: 删除一条被 topic page 引用的记忆后，运行维护，系统报告 broken reference 并给出修复建议（替代记忆 ID 或标记为 broken）。

#### GOAL-maint.4: Duplicate Page Detection [P1]
检测内容高度相似但独立创建的 topic pages（不同 cluster 碰巧描述同一主题）。复用 engram 的 embedding 相似度 + 实体重叠。系统检测到相似度超过可配置阈值（默认 0.85）并标记为疑似重复。

**Pass/fail**: 两个独立 cluster 各自编译了关于同一主题的 page，系统检测到相似度超过阈值并标记为疑似重复。

#### GOAL-maint.5: Knowledge Health Report [P2]
用户可运行知识库健康检查，输出：total pages、stale pages、archived pages、broken links、conflicts、duplicates、coverage（记忆被 page 覆盖的比例）。

**Pass/fail**: 对一个有 10+ topic pages 的知识库运行健康检查，输出包含上述所有指标。对空知识库运行健康检查，输出 total pages = 0 且不报错。

#### GOAL-maint.5b: Maintenance Operation Summary [P1]
每次维护操作（compile、maintenance cycle）输出操作摘要：编译了多少 pages、检测到多少 conflicts/broken links/duplicates、耗时、LLM token cost。

**Pass/fail**: 运行一次 compile 周期后，输出包含上述所有指标的摘要。摘要可通过 CLI `--verbose` 和日志获取。

### §3 — Access & Export（访问和导出）

#### GOAL-maint.6: Knowledge-Aware Recall [P0]
engram recall 时，如果查询匹配某个 topic page，优先返回 topic page 内容（比碎片记忆更有价值）。Topic pages 参与 recall 排序但不替代碎片记忆。

**Pass/fail**: recall("Rust async patterns") 时，如果存在相关 topic page，该 page 出现在结果中且排序权重高于等价的碎片记忆。具体排序策略由 design 决定。

#### GOAL-maint.7: Markdown Export [P0]
一键导出整个知识库为 Markdown 文件夹。每个 topic page 一个 .md 文件，topic 间链接用 `[[wikilinks]]` 格式。输出兼容 Obsidian。空知识库导出时生成空文件夹或仅含 index 文件，不报错。

**Pass/fail**: 导出后在 Obsidian 中打开，Graph View 正确显示 topic 之间的链接关系，每个 page 内容完整可读。

#### GOAL-maint.8: CLI Subcommands [P0]
engram CLI 提供 Knowledge Compiler 子命令，覆盖以下操作：触发编译周期、列出所有 topic pages（含活跃度和状态）、查看单个 topic page 内容、运行健康检查、导出为 Markdown。具体命令名和参数格式由 design 决定。所有子命令全部通过才视为 pass，任一子命令失败即 fail。

**Pass/fail**: 每个操作可通过 CLI 完成且输出人类可读。

#### GOAL-maint.9: Programmatic API [P1]
engram crate 暴露 Knowledge Compiler 的 Rust API，agent 和应用可以程序化调用所有 GOAL-maint.8 中描述的操作。API 设计由 design 决定。

**Pass/fail**: 通过 Rust API 可完成编译、列出 topics、查看单个 topic、健康检查、导出的所有操作。

### §8 — Privacy & Data Security（隐私与数据安全）

#### GOAL-maint.10: Local Data Sovereignty [P0]
本地版所有数据存储在用户本机，不向任何服务器发送数据（LLM API 调用除外）。不含遥测、不含 phone-home、不含匿名使用统计。

**Pass/fail**: 断网环境下（且不配置 LLM），KC 的非编译功能全部可用。代码审查确认无任何非 LLM 的外发网络请求。

#### GOAL-maint.11: LLM Data Transparency [P0]
每次 LLM 调用前，用户可以查看发送给 LLM 的完整 prompt（`--verbose` 或日志）。文档清晰说明哪些操作会调用 LLM、发送什么数据。

**Pass/fail**: `engram compile --verbose` 输出每次 LLM 调用的完整 prompt。README 有 "Privacy" section 说明 LLM 数据流。

#### GOAL-maint.12: DB Encryption (Optional) [P2]
支持可选的数据库 at-rest 加密。加密密钥通过安全渠道获取（环境变量或 keychain），不存在配置文件中。

**Pass/fail**: 启用加密后，DB 文件无法被未授权工具直接读取。密钥获取方式由 design 决定。

---

## Terminology

| Term | Definition |
|---|---|
| **stale** | Needs recompilation — source memories have changed since last compile |
| **archived** | Low activity — still valid content, but rarely accessed. Below activity threshold. |
| **active** | Topic page with healthy activity score, regularly accessed or reinforced |
| **broken** | Topic page references a source memory that no longer exists |
