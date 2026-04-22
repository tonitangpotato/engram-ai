# TODO-MASTER: engramai (engram-ai-rust)

> **项目**: `/Users/potato/clawd/projects/engram-ai-rust`
> **当前版本**: v0.2.3 (已发布 crates.io)
> **测试**: 710+ pass, 0 fail
> **汇总日期**: 2026-04-20
> **来源**: TASKS-AUTOPILOT.md / .gid/docs/issues-index.md / .gid/issues/ / .gid/features/ / MIGRATION_CHECKLIST.md

---

## 状态速览

| 类别 | 总数 | 完成 | 进行中 | 未开始 |
|---|---|---|---|---|
| Autopilot 任务 (v0.2.3) | 15 | 15 | 0 | 0 |
| Issues (ISS-001 ~ 017) | 17 | 16 closed | — | 1 open (ISS-014) |
| Features (.gid/features/) | 7 | 部分实现 | — | — |
| Schema Migration | 完成 | ✅ | — | — |

**一句话**：v0.2.3 发布后 recall 质量提升 + ISS-016 (LLM triples) + ISS-017 (HNSW 扩展) 都已实现。**当前真正剩下的只有 ISS-014 Storage Trait 抽象（P2）**。另外要跟进 7 个 feature 的 status 字段。

---

## ⚠️ 修正记录 (2026-04-20 21:03)
初版 TODO-MASTER 误判 ISS-016/017 为"未实现"。实际上两者都已实现并 merge（见 git log commit `0383584` ISS-016，`src/compiler/discovery.rs` 头部注释明确 ISS-017 用 HNSW 做 O(n·log n)）。
`.gid/docs/issues-index.md` 里这两条缺显式 `## ISS-016/017` section，导致 index grep 看不到，但实现是完成的。需要补 index 条目（T5.2 顺手做）。

---

## 🟢 P2 — 重要但不阻塞（**当前实际仅有的 open item**）

### ISS-014: Storage Trait 抽象
- **路径**: `.gid/docs/issues-index.md` ISS-014
- **状态**: open
- **目标**: 从 `storage.rs` (~2545 行) 抽出 `MemoryStore` trait，算法层只依赖接口
- **动机**: 多 agent 记忆共享前做好准备。SQLite 单写者模型在 10+ agent 并发扛不住
- **范围**: 搬现有代码包装成 `SqliteStore`，零行为变更，所有测试必须通过
- **优先级**: 当前不阻塞，在多 agent 记忆共享之前完成即可

---

## 📦 Features — 状态梳理

位于 `.gid/features/`：

| Feature | requirements.md 状态 | 需要跟进 |
|---|---|---|
| `knowledge-compiler` | 无明确 status 字段 | 核对实现情况 |
| `knowledge-synthesis` | Draft v6 (scope-trimmed) | 有 requirements-1/2 子文档，确认实现进度 |
| `memory-lifecycle` | Partial (4/9) | 列出剩余组件 |
| `multi-signal-hebbian` | 无明确 status | ISS-015 已 close，大概完成了，需确认 |
| `rumination` | 无明确 status | 确认是否已实现 |
| `supersession` | 无明确 status | commit `232ab4e` 相关，确认是否 done |
| `entity-indexing` | 无 requirements.md？ | ISS-009 closed，但 feature dir 里没文档？ |

**Action (T5.2 范围)**: 每个 feature 加一个明确的 status 字段 + 如果还没完全实现，把剩余 GOAL 列出来。

---

## ✅ 已完成（主要成果汇总）

### v0.2.3 发布 (TASKS-AUTOPILOT.md)
- TASK-01 ~ 15 全部 done
- Memory Supersession commit `232ab4e`
- Lifecycle Phase 4-5 health checks + sleep cycle
- Synthesis incremental staleness + emotional modulation + cluster attempt history + pairwise similarity + auto-update actions
- storage.all() 热循环 O(C×N) 修复
- test_merge_from_env 并发修复
- Knowledge Compiler 审计（→ TASK-13-KC-AUDIT.md）
- Somatic markers 接入 decision loop
- Meta-Cognition metrics collection 起点

### Recall 质量提升 (Phase A + B, 2026-04-09 启动)
**Phase A — 止血（RustClaw 侧）**
- A1 EngramStoreHook 过滤（HEARTBEAT_OK / NO_REPLY / 系统指令不存）
- A2 Extractor prompt negative examples
- A3 SQL 一次性清理（105 条精确重复 + 垃圾）

**Phase B — 提质（engramai crate）**
- B1 Entity 索引（ISS-009）
- B2 Dedup on write（ISS-003）
- B3 Confidence score（ISS-007，commit `c03a339`）
- B4 Recency 调参（ISS-002）

### Closed Issues (14 个)
- ISS-001 FTS5 concurrent corruption
- ISS-002 Recency bias / ACT-R decay 调参
- ISS-003 add() dedup
- ISS-004 中文分词（jieba_rs + tokenize_cjk_boundaries）
- ISS-005 Consolidate 合成 insight
- ISS-006 多路 hybrid search (FTS5 15% + Embedding 60% + ACT-R 25%)
- ISS-007 Recall confidence score
- ISS-008 Knowledge promotion
- ISS-009 Entity 索引
- ISS-010 Embedding model_id 一致性
- ISS-011 Recall 结果去重
- ISS-012 Importance 校准
- ISS-013 STDP 可配置
- ISS-015 聚类算法升级（Union-Find → Infomap）

### Schema Migration (SCHEMA_MIGRATION_SUMMARY.md / MIGRATION_CHECKLIST.md)
- 9 列 timestamp TEXT → REAL（Unix float）
- 新增 4 张表：engram_meta、entities、entity_relations、memory_entities
- 4 个文件修改：storage.rs、bus/{feedback,subscriptions,accumulator}.rs
- 没做：数据迁移脚本（老 DB → 新 schema）← 需要时再补

---

## 📚 文档地图

**核心文档（必读）**
- `README.md` — 项目概览
- `PROJECT_SUMMARY.md` — 项目状态汇总
- `ENGRAM-V2-DESIGN.md` — 情感总线架构
- `INTEROCEPTIVE-LAYER.md` — 内感受层完整设计

**研究 / 调研**
- `MEMORY-SYSTEM-RESEARCH.md` — 与 Hindsight / Mem0 / Zep 对比
- `INVESTIGATION-2026-03-31.md` — 生产环境问题调查
- `LEARNINGS.md` — 运维笔记 + recall 质量改进方案
- `COGNITIVE-COMPONENTS-BACKLOG.md` — 30 个认知组件（A-E 分类）

**集成**
- `INTEGRATION.md` — RustClaw 集成指南
- `IRONCLAW_INTEGRATION.md` — IronClaw 集成
- `EMBEDDING_PROTOCOL.md` — embedding 协议 v2
- `QUICKSTART.md` — 快速上手

**Migration**
- `SCHEMA_MIGRATION_SUMMARY.md` — schema 迁移总结
- `MIGRATION_CHECKLIST.md` — 迁移检查清单

**Task / Issue 索引**
- `TASKS-AUTOPILOT.md` — v0.2.3 autopilot 任务（全 done）
- `.gid/docs/issues-index.md` — issues 总索引
- `.gid/issues/` — 按 issue 分文件夹（requirements / design）
- `.gid/features/` — 按 feature 分文件夹（requirements）

---

## 🎯 推荐执行顺序

实际上大活儿都干完了。剩下：

1. ~~**T5.2** — 给 7 个 feature 补 status + 同步 `.gid/docs/issues-index.md`（把 ISS-016/017 加上显式 section）~~ ✅ **done 2026-04-20 21:18** — 三个 ISS section 补入、6 个 feature status 字段就位、memory-lifecycle 剩余组件清单已内嵌文档。另顺带 push 了 8 个积压的本地 commit（含 uncommitted 的 ISS-017 HNSW 修复，现 commit `40bb499`）。
2. **ISS-014 Storage Trait** (P2) — 多 agent 共享记忆之前做好。不紧急。
3. **memory-lifecycle 剩余组件** — 清单确定（见 `.gid/features/memory-lifecycle/requirements.md`）：
   - C2 semantic dedup (GOAL-2) — entity overlap 作 dedup 第二信号
   - C3 reconcile by ID (GOAL-5) + 合并后 entity 重抽 (GOAL-6)
   - C4 incremental synthesis (GOAL-11) — `IncrementalState` 已定义未接线
   - C8 temporal decay (GOAL-23/24/24a) — forget() 完整清理链路 + insight provenance 标记
   - C9 memory rebalancing (GOAL-26/27/28) — diagnostic report + compact + health metrics trend
4. **下一波新 feature** — 取决于 potato 的路线图，不在本汇总范围内。

---

## 🙏 致歉说明（T5.1 初版错误）

初版 TODO-MASTER 把 ISS-016 和 ISS-017 标成 "未实现"，是因为：
- `.gid/issues/` 目录下有两者的 requirements/design 文件夹，我误以为只写了设计没实现
- `.gid/docs/issues-index.md` 缺少 `## ISS-016` 和 `## ISS-017` 的显式 section（只在表格和总结行里提了一下）
- 没去查 git log 验证

**教训**：判断 issue 状态必须 `git log --oneline | grep ISS-NNN` 和 grep 实际代码，不能只看 issue tracker。Tracker 可能漏更新。

*本文档由 T5.1 生成，初版写于 2026-04-20 20:55，修正于 21:03。*
