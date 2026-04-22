# Requirements: Platform Setup, LLM Config, Import & Intake

This feature covers the platform infrastructure layer: multi-provider LLM configuration, zero-config setup with embedding auto-install, embedding fallback chains, standalone product installation, feature flag architecture, data import pipelines (Markdown, Obsidian, URL, bookmarks), and intake channels (directory watch, voice intake, browser extension). See [master requirements.md](../requirements.md) for GUARDs (system-wide constraints GUARD-1 through GUARD-6).

> **Note**: Section numbers (§5, §5.5, §6, §9) follow master requirements numbering. Other sections are in other feature documents.

### Dependencies

- GOAL-plat.5 (embedding fallback) depends on GOAL-plat.4 (embedding runtime setup) for the primary local provider.
- GOAL-plat.9 (Obsidian import) depends on GOAL-plat.8 (Markdown import) — Obsidian is a superset of Markdown.
- GOAL-plat.10 (URL import) and GOAL-plat.11 (bookmarks import) share HTTP fetching capability.
- GOAL-plat.13 (voice intake) depends on GOAL-plat.12 (directory watch) for the inbox mechanism.

### Out of Scope

- Cloud-hosted KC deployment
- Mobile app intake channels
- Real-time collaborative editing of knowledge
- Sync between multiple KC instances

---

## GOALs — Functional Requirements

### §5 — LLM Configuration & Provider Management（LLM 配置）

#### GOAL-plat.1: Multi-Provider LLM Support [P0]
KC 支持多个 LLM provider：Anthropic Claude、OpenAI GPT、本地模型（Ollama / OpenAI-compatible API）。用户通过配置文件选择 provider 和 model。

**Pass/fail**: 用户配置本地模型 provider 后，编译操作成功使用本地模型生成 topic page。同样配置 Anthropic 和 OpenAI 也能工作。

#### GOAL-plat.2: LLM Configuration File [P0]
KC 独立产品使用单一配置文件管理所有运行时设置，包括：LLM provider 选择、API 凭证、模型名称、自定义 endpoint（用于 Ollama/Azure/代理）、embedding provider（独立于 LLM provider）、embedding 模型选择、自动编译间隔、数据库路径。本地模型的 API 凭证可省略。

**GUARDs**: GUARD-4 (API 凭证不泄露到日志或导出)

**Pass/fail**: 首次初始化生成默认配置文件，用户编辑后编译操作读取配置并使用指定 provider。配置缺失必填项时报清晰错误信息。

#### GOAL-plat.3: Graceful LLM Degradation [P1]
当 LLM 不可用（无 API key、provider 离线、配额耗尽）时，KC 的非 LLM 功能正常工作：存储、recall、Markdown 导出、健康检查（不含冲突检测）。只有编译和冲突检测需要 LLM。

**Pass/fail**: 不配置任何 LLM API key 时，记忆存储、recall、topic 列表、知识库导出正常工作。编译操作报错提示需要 LLM 配置，但不崩溃。

### §5.5 — Setup & Embedding Strategy（安装与 Embedding 策略）

#### GOAL-plat.4: Zero-Config Semantic Search Setup [P0]
初始化流程自动检测本地 embedding 运行时是否可用。如果未安装，提示用户确认后自动安装适合当前操作系统的版本。安装后自动启动服务、拉取默认 embedding 模型、并验证 embedding 生成可用。如果用户所在平台不支持自动安装，提供清晰的手动安装指引。

**GUARDs**: GUARD-5 (自动安装中途失败不丢失已有数据)

**Pass/fail**: 在一台未安装 embedding 运行时的机器上运行初始化，用户确认后运行时安装完成、模型就绪、测试 embedding 返回成功。在不支持自动安装的平台上，输出清晰的手动安装步骤。

**Error handling**: 安装失败时清晰报错并告知用户手动安装方式，不阻塞其他功能。

#### GOAL-plat.5: Embedding Provider Fallback Chain [P0]
Embedding provider 按优先级自动选择：
1. 用户配置的本地 embedding provider（免费，推荐）
2. 用户配置的云端 embedding API（收费）
3. **无 embedding 时明确警告用户 recall 质量将显著下降**，降级到纯关键词匹配

系统在启动时检测可用 provider 并记录选择原因。

**Architectural constraint — single embedding source**: KC 的所有子系统（topic discovery、compilation、cross-topic linking）使用 engram core 已经存好的 embedding（`memory_embeddings` 表）。KC 不应维护自己独立的 embedding provider 或 embedding cache 来处理源记忆。KC 可以为 topic page 本身生成 embedding（用于 topic 间相似度计算），但源记忆的 embedding 必须来自 engram core 的统一管道。这避免了同一 crate 内两套 embedding 系统导致的维度不匹配、模型不一致、重复计算等问题。

**Pass/fail**: (1) 本地 provider 配置且可用 → 使用之，日志显示所用 provider 和模型。(2) 本地不可用但有云端 key → 使用云端，日志显示 provider 切换原因。(3) 两者都不可用 → 启动成功但输出明确警告，说明 recall 将降级为关键词匹配。

#### GOAL-plat.6: Standalone Product Installation [P0]
KC 作为独立产品可在 macOS 和 Linux 上安装使用，不依赖 RustClaw。支持通过包管理器、预编译 binary、或从源码编译安装。安装后初始化流程引导用户完成首次配置（数据库路径、LLM provider、embedding 运行时安装）。

**Pass/fail**: 在一台干净机器上安装后，初始化 → 添加测试记忆 → recall 完整流程可用。

#### GOAL-plat.7: Feature Flag Architecture [P1]
engram crate 通过编译时 feature flags 分层，使得 KC 独立产品只包含知识相关功能（存储、recall、embedding、FTS5、实体抽取、synthesis、KC 编译），而 agent 框架可以 opt-in 额外能力（会话工作记忆、LLM 抽取器、查询分类、异常检测、情感分析等）。具体 feature flag 名称和分组由 design 决定。

**Pass/fail**: 仅启用默认 features 编译成功，binary 不包含 agent 专用模块代码。启用全部 features 也编译成功且包含全部功能。

### §6 — Data Import & Cold Start（数据导入与冷启动）

#### GOAL-plat.8: Markdown Batch Import [P0]
用户可将 Markdown 文件夹批量导入为 engram 记忆。每个 .md 文件成为一条或多条记忆（按标题拆分 sections）。保留文件名和目录结构作为 metadata/tags。

**Pass/fail**: 一个包含 20 个 .md 文件的文件夹，批量导入后产生 ≥20 条记忆，每条记忆的 metadata 包含源文件路径。编译后这些记忆被正确编译成 topic pages。

**Error handling**: 空文件跳过并记录，非 UTF-8 文件报错并继续处理其余文件，导入操作输出摘要（成功/跳过/失败各多少）。

**Progress**: 批量导入超过 10 个文件时输出进度信息。

#### GOAL-plat.9: Obsidian Vault Import [P1]
支持从 Obsidian vault 导入，额外处理 Obsidian 特有格式：
- `[[wikilinks]]` → 导入后保留为记忆间的 Hebbian links
- YAML frontmatter → 提取为记忆 metadata（tags、dates、aliases）
- `![[embeds]]` → 解析被嵌入文件并关联

**Pass/fail**: 导入一个有 10 个 notes 且 notes 间有 `[[wikilinks]]` 的 Obsidian vault，导入后记忆间存在对应的 Hebbian links，来源 vault 的图谱结构在 engram 中得以保留。

**Error handling**: 同 GOAL-plat.8，malformed frontmatter 跳过并警告，不阻塞其余文件导入。

#### GOAL-plat.10: URL Batch Import [P1]
用户可提供 URL 列表文件，系统批量抓取内容并导入为记忆。支持可配置限速（防止被目标网站封 IP）。

**Note**: URL 抓取是用户主动发起的入站数据流（external → local），不违反 GOAL-maint.10 的本地数据主权原则（Local Data Sovereignty）。

**Pass/fail**: 一个含 10 个 URL 的文本文件，批量导入后成功抓取并导入 ≥8 条记忆（允许部分 URL 失败），失败的 URL 报告在输出中。

#### GOAL-plat.11: Browser Bookmarks Import [P2]
支持从浏览器导出的书签文件（Chrome/Firefox 标准格式）导入。提取 URL + 标题 + 文件夹结构作为 tags。

**Pass/fail**: 导入浏览器导出的书签文件，书签的文件夹层级被保留为 tags，URL 被抓取内容后存为记忆。

#### GOAL-plat.15: Import Progress & Error Reporting [P1]
所有批量导入操作（Markdown、Obsidian、URL、bookmarks）输出一致的操作摘要：总文件数、成功数、跳过数、失败数（含失败原因）、耗时。大批量导入（>10 项）时实时输出进度。

**Pass/fail**: 导入一个含有正常文件、空文件和损坏文件的混合文件夹，输出包含上述所有指标的摘要，失败项列出具体原因。

#### GOAL-plat.16: Config Migration [P1]
当配置文件 schema 在版本升级间变化时，系统自动检测旧版 schema 并提示迁移（或自动迁移 + 备份旧配置）。不静默丢弃旧配置项。

**Pass/fail**: 升级 engram 版本后，旧配置文件被检测到并迁移，旧配置备份保留，新增配置项使用默认值。

### §9 — Intake Channels（Intake 渠道）

#### GOAL-plat.12: Directory Watch Intake [P0]
KC daemon 监听配置的 inbox 目录。用户放入支持格式的文件（.md / .txt 等），daemon 自动读取内容、导入为记忆、处理后将文件移出 inbox（移到已处理子目录）。

**GUARDs**: GUARD-5 (文件处理失败不丢失原文件)

**Pass/fail**: daemon 运行时，将一个文件放入 inbox 目录，5 秒内文件被处理成 engram 记忆，文件被移至已处理子目录。

**Error handling**: inbox 目录不存在时自动创建。文件权限错误时记录警告并跳过该文件（不删除）。处理失败的文件移到 error 子目录而非删除。

#### GOAL-plat.13: Voice Intake (Local) [P0]
CLI 和 daemon 支持语音 intake：用户放 .ogg/.wav/.mp3 文件到 inbox 目录，或通过 CLI 指定音频文件，系统 STT 转文字后导入为记忆。

**Pass/fail**: 将一个 60 秒的语音文件放入 inbox 目录，daemon 将其转录为文字并存为记忆。转录文本与原始语音内容语义一致（人工抽检通过）。

#### GOAL-plat.14: Browser Extension [P2]
提供浏览器扩展（Chrome/Firefox），用户在网页上点击按钮或右键菜单，将当前页面内容/选中文本发送到本地 KC daemon 的 HTTP 端口。

**Pass/fail**: 安装扩展后，用户在任意网页点击保存按钮，页面标题 + URL + 正文被发送到本地 daemon 并存为记忆。
