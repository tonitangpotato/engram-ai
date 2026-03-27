# Engram Roadmap

> Universal cognitive memory for AI agents — any language, any bot, full capabilities.

**Date**: 2026-03-22
**Status**: Active

---

## The Big Picture

Engram 要解决的核心问题：**任何 AI agent，不管用什么语言写的，都能共享同一套认知记忆系统，并使用全部功能。**

---

## Problem Map

### 1. Unified Schema (跨语言共享)
**现状**: Python/Rust/TS 各自定义 schema，互不兼容
**目标**: 一个 SQLite 文件，所有语言都能读写
**Design**: `DESIGN-unified-schema.md`
**Tasks**:
- [ ] Finalize canonical schema v1
- [ ] Migration script for existing Python DB (889 memories)
- [ ] Update Python `store.py`
- [ ] Update Rust `storage.rs`
- [ ] Update TS `store.ts`
- [ ] Cross-language integration test

### 2. Integration Interfaces (bot 如何接入)
**现状**: CLI 是唯一通用接口，但有 90ms 启动开销且功能不完整
**目标**: 每种集成方式都能 perform 全部功能

| Interface | 适用场景 | 全功能? | 状态 |
|-----------|---------|---------|------|
| **Python SDK** | Python bot 直接 import | ✅ | 最完整 |
| **Rust crate** | Rust bot (RustClaw) | ⚠️ 缺 embedding, export, hebbian query | 需补齐 |
| **TS/npm** | Node.js bot | ⚠️ 未验证 v1 兼容性 | 需审查 |
| **CLI** | 任何 bot via exec | ⚠️ 缺部分命令 | 需补齐 |
| **MCP** | Claude Code / Cursor / etc | 🔧 实现中 | subagent building |
| **HTTP API** | 远程/微服务 | ❌ 不存在 | 低优 |

#### CLI 需要补齐的命令
- [ ] `engram update <id> <new_content>` — 修改记忆
- [ ] `engram pin/unpin <id>` — 固定记忆
- [ ] `engram reward <feedback>` — 强化最近记忆
- [ ] `engram hebbian <query>` — 已有，验证功能
- [ ] `engram search` — 纯 FTS5 搜索（vs recall 的 ACT-R 加权）
- [ ] `engram info <id>` — 查看单条记忆详情（activation, links, access history）

#### Rust crate 需要补齐
- [ ] Embedding support (call Ollama HTTP or bundle ONNX)
- [ ] `export()`
- [ ] `update_memory()`
- [ ] `hebbian_links()` query API
- [ ] `recall_causal()`
- [ ] Timestamp format: TEXT → REAL (unified schema)

#### MCP Server
**Decision**: 实现。MCP 是最通用的 bot 集成方式。
- Claude Code, Cursor, Windsurf, Codex 原生支持
- 比 CLI exec 更 structured（JSON schema input/output）
- 实质上是 Python SDK 的薄包装，维护成本低
- 命令: `engram mcp` 启动 stdio MCP server

### 3. Version Synchronization (版本同步)
**现状**: 5 个发布渠道，手动同步
**目标**: 统一版本号，CI 自动发布

| Package | Registry | Current | Target |
|---------|----------|---------|--------|
| `engramai` | PyPI | 1.1.0 | 1.2.0 (schema v1) |
| `neuromemory-ai` | npm | ? | 1.2.0 |
| `engramai` | crates.io | ? | 1.2.0 |
| `engram` | CLI (pip) | 2.0.0 local | 1.2.0 |
| OpenClaw plugin | local | 1.0.0 | defer to v2 |

**Tasks**:
- [ ] Align all versions to 1.2.0 for schema v1 release
- [ ] Monorepo or workspace?
  - Option A: Keep separate repos, sync manually
  - Option B: Monorepo with Python + Rust + TS in one repo
  - Option C: Keep repos separate, add CI to cross-test
- [ ] PyPI publish workflow (GitHub Actions)
- [ ] npm publish workflow
- [ ] crates.io publish workflow
- [ ] Version bump script that updates all 3

### 4. Feature Parity Matrix
**目标**: 所有接口支持相同的核心功能

| Feature | Python | Rust | TS | CLI |
|---------|--------|------|----|-----|
| add | ✅ | ✅ | ✅ | ✅ |
| recall (FTS5) | ✅ | ✅ | ✅ | ✅ |
| recall (embedding) | ✅ | ❌ | ❌ | ✅ |
| recall (hybrid) | ✅ | ❌ | ❌ | ✅ |
| consolidate | ✅ | ✅ | ✅ | ✅ |
| forget | ✅ | ✅ | ? | ✅ |
| reward | ✅ | ✅ | ? | ❌ |
| downscale | ✅ | ✅ | ? | ❌ |
| pin/unpin | ✅ | ✅ | ? | ❌ |
| export | ✅ | ❌ | ? | ✅ |
| update | ✅ | ❌ | ? | ❌ |
| hebbian links | ✅ | config only | ? | ✅ |
| namespace | ✅ | ✅ | ? | ❌ |
| ACL | ✅ | ✅ | ? | ❌ |
| embedding store | ✅ | ❌ | ❌ | ✅ |
| emotional bus | ✅ | ✅ | ✅ | ❌ |
| init/status | ✅ | N/A | N/A | ✅ |

---

## Priority Order

1. **Unified schema v1** — foundation, everything else depends on this
2. **Python CLI completion** — most used interface, needs full features
3. **Rust crate alignment** — for RustClaw migration
4. **PyPI 1.2.0 publish** — get the fixes out to the world
5. **Cross-language test** — prove it works
6. **npm/crates.io publish** — after testing
7. **MCP server** — if/when needed

---

## Non-Goals (for now)

- Distributed/networked memory (Redis, Postgres backend)
- Real-time sync between multiple DB files
- GUI memory editor (Pensieve is separate project)
- v2 features (Emotional Bus, ACL, Subscriptions) — stabilize v1 first

---

*This roadmap guides Engram from "works for me" to "works for everyone."*
