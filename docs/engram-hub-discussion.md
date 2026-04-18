# Engram Hub — Agent Experience Sharing Platform

> Discussion doc. 待后续详细讨论后写 requirements。

**Date**: 2026-04-06
**Status**: Early discussion
**Related**: IDEA-20260406-02 (Sharable Memories), IDEA-20260406-03 (this), IDEA-20260405-01 (Engram Protocol)

---

## 1. 核心定位

**Engram Hub = Agent 经验的 GitHub**

Agent 在工作中积累的 debug 经验、最佳实践、领域知识，不应该锁死在单个 agent 的 SQLite 里。Engram Hub 让 agent 的经验可以：
- **发布** — 按领域 tag 导出，sanitize 后上传
- **发现** — 按领域浏览/搜索经验包
- **安装** — 一键导入到本地 engram DB
- **评价** — 基于实际使用效果自动打分

不是在建 RAG 数据库。是在建**认知经验的 package manager**。

---

## 2. 产品形态

### 用户视角

```
# 发布经验
$ engram publish --tags "pytorch,model-training,debugging" --name "ml-training-debug-v1"
🔍 Scanning memories... found 847 matching
🔒 Sanitizing... removed 23 PII entries, 45 personal memories
📦 Packaged 779 memories + 1,203 Hebbian links
🚀 Published to hub.engram.dev/potato/ml-training-debug-v1

# 安装经验
$ engram install potato/ml-training-debug-v1
📥 Downloading... 779 memories, 1,203 links
🔑 Trust level: 0.7 (community package)
✅ Imported. Your agent now has ML training debug experience.

# 搜索
$ engram search "kubernetes debugging"
  @alice/k8s-debug-pro       ★ 4.8  (312 installs)
  @bob/cloud-native-ops      ★ 4.5  (89 installs)
  @devops-guild/k8s-bible    ★ 4.9  (1,204 installs)
```

### Web 视角

```
hub.engram.dev
├── Explore — 按领域浏览 trending packages
├── @username — 个人 profile + 发布的 packages
├── package page — README, stats, version history, reviews
└── Organizations — 团队共享 private packages
```

---

## 3. 数据模型

### Experience Package（云端存储格式）

```
ml-training-debug-v1/
├── manifest.json      # 元数据、版本、tags、作者、license
├── memories.jsonl     # 过滤后的记忆（标准 engram schema）
├── links.jsonl        # Hebbian 关联关系
├── README.md          # 人类可读描述（自动生成 + 手动编辑）
└── stats.json         # 记忆数量、覆盖领域、质量指标
```

### manifest.json

```json
{
  "name": "ml-training-debug-v1",
  "version": "1.0.0",
  "author": "potato",
  "description": "779 debug experiences from 6 months of ML model training",
  "tags": ["pytorch", "model-training", "debugging", "cuda", "distributed"],
  "memory_count": 779,
  "link_count": 1203,
  "license": "MIT",
  "engram_version": ">=0.3.0",
  "created": "2026-04-06T17:00:00Z",
  "sanitized": true,
  "privacy_filters": ["no_emotional", "no_relational", "no_pii"]
}
```

### memories.jsonl（每行一条记忆）

```json
{
  "id": "uuid",
  "content": "PyTorch shape mismatch: when using mixed precision, ensure loss.backward() receives a scalar, not a tensor. Use loss.mean().backward() if loss is multi-element.",
  "memory_type": "procedural",
  "importance": 0.85,
  "tags": ["pytorch", "mixed-precision", "debugging"],
  "created": "2026-03-15T10:30:00Z",
  "access_count": 12,
  "source_agent": "potato/rustclaw"
}
```

注意：不含 embedding vectors。导入方用自己的 embedding model 重新生成（避免模型版本不兼容）。

### links.jsonl（Hebbian 关联）

```json
{
  "from_content_hash": "abc123",
  "to_content_hash": "def456",
  "strength": 0.73,
  "co_occurrence_count": 8,
  "context": "gradient issues often co-occur with mixed precision"
}
```

用 content hash 而不是 UUID，这样导入方可以匹配已有记忆。

---

## 4. 云端架构

### 最小架构（Phase 1）

```
Client (engram CLI)
    │
    ▼  HTTPS
┌────────────────────┐
│ Cloudflare Workers  │  ← API layer (publish, install, search, auth)
│ (or Axum on Fly.io) │
└────────┬───────────┘
         │
    ┌────┴────┐
    │         │
    ▼         ▼
┌────────┐ ┌──────────────┐
│ R2/S3  │ │ Turso/Postgres│
│(blobs) │ │  (metadata)   │
└────────┘ └──────────────┘
  packages    users, packages index,
  .jsonl      ratings, download counts
```

**为什么 Cloudflare Workers + R2：**
- 全球边缘部署，低延迟
- R2 无 egress 费用（package 下载免费）
- Workers 免费 tier 足够 MVP
- 后续可以迁到自建

**为什么 Turso：**
- SQLite on the edge，和 engram 本地 SQLite 天然兼容
- 或者直接 Postgres（更通用）

### API 设计（初步）

```
POST   /v1/packages                    # publish
GET    /v1/packages?tags=pytorch       # search
GET    /v1/packages/:owner/:name       # package info
GET    /v1/packages/:owner/:name/dl    # download
DELETE /v1/packages/:owner/:name       # unpublish
POST   /v1/packages/:owner/:name/rate  # rate
GET    /v1/users/:username             # profile
```

Auth: GitHub OAuth → JWT tokens

---

## 5. 安全 & 隐私

### 发布前 Sanitization（关键）

这是平台能否被信任的核心。发布前必须：

1. **过滤 memory_type**：`emotional`, `relational` 默认不可导出
2. **PII 扫描**：正则 + LLM 检测文件路径、API keys、密码、邮箱、IP 地址
3. **内容审查**：不能发布包含他人隐私信息的经验
4. **用户确认**：publish 前显示将要上传的内容摘要，用户明确确认

```
$ engram publish --tags "pytorch" --name "my-pkg"
🔍 Found 847 matching memories
🔒 Filtered:
   - 23 emotional memories (removed)
   - 12 relational memories (removed)
   - 8 containing file paths (sanitized: paths replaced with <PATH>)
   - 5 containing API keys (removed)
📦 Ready to publish: 799 memories

Preview first 5:
  1. "PyTorch shape mismatch: when using mixed precision..."
  2. "CUDA OOM at batch_size=64: reduce to 32 or use gradient..."
  ...

Publish? [y/N]
```

### 导入安全

- **Trust scoring**：导入的记忆初始 importance 打折（源头 0.8 → 导入后 importance × trust_factor）
- **Isolation**：imported 记忆标记 `source` 字段，可随时批量删除
- **Quarantine**：可选 — 导入后先隔离，使用一段时间后再提升 trust

---

## 6. 社区机制

### Rating 系统

**不只是人工打分，更重要的是自动质量信号：**

- 📊 **Recall hit rate** — 导入后，这些记忆被 agent recall 实际命中的比率
- ⏱️ **Time to resolution** — 导入经验包后，相关任务完成时间是否缩短
- 👍 **Manual rating** — 用户显式打分
- 🔄 **Retention** — 用户导入后有没有删除

### Discovery

- **Trending** — 近期 install 数增长最快
- **Curated collections** — "Best for ML beginners", "Senior Rust developer's toolkit"
- **Recommended** — 基于你已有记忆的 tag 分布推荐互补经验包
- **Organizations** — 公司/团队可以维护 private registry

### Fork & Improve

```
$ engram fork alice/k8s-debug-pro
📥 Forked to potato/k8s-debug-pro (based on alice/k8s-debug-pro v2.1)
# ... 使用一段时间，积累自己的 k8s 经验 ...
$ engram publish --name k8s-debug-pro --base alice/k8s-debug-pro
🚀 Published potato/k8s-debug-pro v1.0 (forked from alice/k8s-debug-pro)
```

---

## 7. 商业模式

| Tier | Price | Features |
|------|-------|----------|
| Free | $0 | Publish public packages, install public packages, 5 packages limit |
| Pro | $10/mo | Unlimited packages, private packages, analytics dashboard, priority search |
| Team | $25/seat/mo | Shared private registry, access control, audit log |
| Enterprise | Custom | Self-hosted registry, SSO/SAML, SLA, dedicated support |

**Marketplace cut**: 付费经验包抽 20%（作者设价，平台抽成）

**Revenue projections (conservative)**:
- Y1: Focus on traction, mostly free users. $2-5K MRR from early Pro adopters.
- Y2: Marketplace launches. Pro + marketplace = $20-50K MRR.
- Y3: Enterprise deals. $100K+ MRR possible.

---

## 8. 最小可行路径（Phase 1）

**目标：验证人们是否愿意共享和使用 agent 经验**

1. **engram crate 加 export/import** [P0]
   - `engram export --tags "..." --sanitize --output bundle.jsonl`
   - `engram import bundle.jsonl --trust 0.7 --source "author/name"`
   - 先做本地文件级别，不需要云端

2. **简单 Hub API** [P1]
   - Cloudflare Workers + R2
   - publish / install / search / list
   - GitHub OAuth

3. **Landing page** [P1]
   - hub.engram.dev
   - "Share your agent's hard-won experience"
   - Package browser

4. **CLI integration** [P1]
   - `engram publish` / `engram install` 直接和 Hub 交互

**Phase 1 不需要：**
- 复杂的 rating 系统（先用 download count）
- Fork 功能
- Organizations / teams
- Marketplace 付费

---

## 9. 和 Engram 生态的关系

```
Engram Ecosystem
│
├── engram crate (已有) ─────── 单 agent 认知记忆
│     ├── ACT-R activation
│     ├── Hebbian learning
│     ├── Consolidation
│     └── Surprise detection
│
├── Engram Protocol (IDEA-20260405-01) ── 标准化记忆格式
│     ├── Memory schema
│     ├── Exchange format
│     └── Cross-LLM compatibility
│
├── Sharable Memories (IDEA-20260406-02) ── export/import 能力
│     ├── Field-scoped export
│     ├── Sanitization pipeline
│     └── Trust-scored import
│
├── Engram Hub (IDEA-20260406-03, 本文) ── 社区平台
│     ├── Package registry
│     ├── Discovery & rating
│     └── Marketplace
│
└── Knowledge Compiler (IDEA-20260403-02) ── 知识产品化
      ├── Web UI for knowledge management
      └── Personal cognitive dashboard
```

每一层独立有价值，但组合起来是完整的**认知基础设施**。

---

## 10. Seed Strategy — 从互联网抓取 agent 经验种子数据

### 核心洞察

**不需要等用户来贡献。** 互联网上已经存在海量的"agent 经验"——只是以人类可读的形式散落在各个论坛、问答站、讨论区。我们可以把这些抓取下来，转化为 engram 记忆格式，作为 Hub 的种子数据。

**本质上是把模型的 semantic-level indexed search，变成分领域的 SQLite search。**

现在 LLM 做"搜索"：用户问问题 → embedding → 在巨大向量空间里找最近邻。问题：
- 领域混杂（Rust debug 经验和 Python 入门混在一起）
- 没有认知权重（一条 Stack Overflow 高赞答案和一条垃圾回答同等对待）
- 无法增量更新（模型 training cutoff 之后的知识不存在）
- 无记忆衰减（过时的经验不会降权）

Engram Hub 的方案：**每个领域一个 SQLite 经验库**，带 ACT-R 激活、Hebbian 关联、importance scoring。不是在模型 embedding 空间里搜，是在结构化的认知记忆里搜。

### 数据源 & 抓取策略

```
数据源              │ 目标领域                    │ 记忆类型
────────────────────┼────────────────────────────┼──────────────
Reddit              │ r/rust, r/python,           │ procedural
  /r/MachineLearning│ r/MachineLearning,          │ (debug 经验)
  /r/devops         │ r/devops, r/kubernetes      │ factual (知识)
────────────────────┼────────────────────────────┼──────────────
Stack Overflow      │ 高赞答案 by tag:            │ procedural
                    │ [rust] [pytorch] [k8s]      │
────────────────────┼────────────────────────────┼──────────────
CSDN / 知乎          │ 中文技术社区                  │ procedural
                    │ 机器学习、后端开发              │ factual
────────────────────┼────────────────────────────┼──────────────
Hacker News         │ 技术讨论 (Show HN,           │ opinion
                    │ 深度技术帖)                   │ factual
────────────────────┼────────────────────────────┼──────────────
HuggingFace Forums  │ ML/NLP/LLM 具体问题          │ procedural
  + GitHub Issues   │ 开源项目的 debug trail        │
────────────────────┼────────────────────────────┼──────────────
Discord servers     │ Rust (官方), PyTorch,        │ procedural
                    │ LangChain 等技术 Discord      │ factual
```

### 转化流程

```
Raw Source (Reddit post, SO answer, GitHub issue)
    │
    ▼  Extraction
┌────────────────────────────────────┐
│ 1. 抓取原始内容                      │
│ 2. LLM 提取结构化经验                │
│    - 问题是什么？                    │
│    - 解决方案是什么？                 │
│    - 哪些关键 insight？              │
│ 3. 生成 engram 记忆条目              │
│    - content: 精炼后的经验描述        │
│    - memory_type: procedural/factual │
│    - tags: [rust, async, tokio, ...]│
│    - importance: 基于投票/赞数计算    │
└────────────┬───────────────────────┘
             │
             ▼  Quality Scoring
┌────────────────────────────────────┐
│ importance 计算:                    │
│   SO: score/100 * 0.8 + accepted   │
│   Reddit: upvotes/max_upvotes      │
│   GH Issues: 参与人数 + 是否 resolved│
│   HN: points/max_points            │
│ 过滤: importance < 0.2 → 丢弃      │
└────────────┬───────────────────────┘
             │
             ▼  Domain Packaging
┌────────────────────────────────────┐
│ 按领域打包:                         │
│   @engram-hub/rust-async-debug     │
│   @engram-hub/pytorch-training     │
│   @engram-hub/k8s-troubleshooting  │
│   @engram-hub/react-patterns       │
│ 每个 package = 一个领域的 SQLite     │
│ 带 Hebbian links (共现的经验关联)    │
└────────────────────────────────────┘
```

### vs 传统 Semantic Search 的优势

| 维度 | Semantic Search (RAG) | Engram Domain SQLite |
|------|----------------------|---------------------|
| 索引 | 全局 embedding 空间 | 分领域 SQLite + FTS5 |
| 权重 | 纯余弦相似度 | ACT-R activation (使用频率 + 时间衰减 + importance) |
| 关联 | 无 | Hebbian links (经验之间的共现强化) |
| 更新 | 重新 embed 整个 corpus | 增量写入新记忆 |
| 过时处理 | 无（旧知识永远在） | 自然衰减 (decay) + consolidation |
| 精度 | 混杂无关结果 | 领域内搜索，噪音小 |
| 成本 | 每次 query 调 embedding API | 本地 SQLite query, 零成本 |

**关键差异：Engram 不只是 "存了数据"，而是 "有认知模型的数据"。**

### Official Seed Packages（第一批）

Phase 1 我们自己生成并发布的种子包：

```
@engram-hub/rust-async          # Rust async/await, tokio, 并发问题
@engram-hub/rust-lifetime       # 生命周期、借用检查器常见问题
@engram-hub/pytorch-training    # 模型训练 debug (OOM, NaN, shape mismatch)
@engram-hub/pytorch-deployment  # 模型部署 (ONNX, TensorRT, quantization)
@engram-hub/k8s-debugging       # Kubernetes 排障
@engram-hub/docker-ops          # Docker 构建、多阶段、优化
@engram-hub/git-advanced        # Git 高级操作、rebase、cherry-pick
@engram-hub/linux-admin         # Linux 运维常见问题
@engram-hub/llm-prompting       # LLM prompt engineering 最佳实践
@engram-hub/agent-patterns      # AI agent 开发 patterns
```

每个包 500-2000 条记忆，来源标注清楚（SO/Reddit/GH），importance 基于社区投票。

### Cold Start 解决

这个 seed strategy 解决了平台最大的鸡生蛋问题：

1. **Day 1 就有内容** — 不需要等用户贡献
2. **高质量起点** — 来源是 SO 高赞答案、Reddit 高票帖子，比随便一个用户的经验可靠
3. **示范效果** — 官方 seed packages 展示了 "好的经验包长什么样"
4. **引导社区** — 用户看到 @engram-hub/rust-async 有 1500 条记忆，会想 "我的 Rust 经验也可以这样分享"

### 抓取管道（技术实现）

```
Scraper (xinfluencer 的 crawler 复用)
    │
    ├── Reddit API (已有)
    ├── Stack Overflow API (data dump 也行)
    ├── GitHub Issues API
    ├── HN Firebase API (已有，social-intake skill)
    ├── CSDN (Jina Reader)
    └── 知乎 (Jina Reader)
    │
    ▼
LLM Pipeline (batch processing, 用便宜模型)
    │ 每条原始内容 → 1-3 条 engram 记忆
    │ Sonnet 4.5 / Haiku 4 足够
    ▼
Quality Filter
    │ importance < 0.2 → drop
    │ duplicate detection (content hash)
    ▼
Domain Packager
    │ 按 tag 分组 → package
    │ 生成 Hebbian links (同 package 内相关记忆)
    ▼
Publish to Hub
```

### 成本估算

- Reddit/SO/GH API: 免费 tier 足够 seed 阶段
- LLM 处理 10K 条原始内容 → ~30K 记忆: 
  - Haiku 4: ~$3-5 (0.25/M input, 1.25/M output)
  - Sonnet 4.5: ~$15-20
- 存储: negligible (JSONL 很小)
- **总 seed 成本: <$50 就能生成 10 个领域 × 1000+ 记忆的种子库**

---

## 11. Open Questions（待讨论）

1. **Package 粒度** — 一个 package 应该多大？100 条记忆？1000 条？有没有上限？
2. **版本更新** — 作者更新 package 后，已安装的用户怎么通知？自动更新还是手动？
3. **质量控制** — 谁来保证 package 质量？纯社区驱动还是需要 curation？
4. **Embedding 兼容性** — 不同 agent 用不同 embedding model，import 时重新生成 embedding 是否够？
5. **Cross-agent memory format** — 非 engram agent（用 Mem0、Zep 等）能否导入 engram packages？需要 adapter？
6. **Legal** — 谁拥有 agent 产生的记忆的知识产权？用户？agent 运营者？
7. **Abuse** — 如何防止 SEO spam（发布大量低质量 package 刷排名）？
8. **Offline / Air-gapped** — 企业客户可能不能连外网，需要支持 private registry mirror

---

## 12. Engram vs Knowledge Graph — 数据模型选择

### 问题

从互联网抓取的结构化知识，应该用什么模型存？三个选项：

| 方案 | 模型 | 优点 | 缺点 |
|------|------|------|------|
| A. 纯 Engram | flat text + importance + Hebbian co-occurrence | 简单，已有 | 没有因果关系 |
| B. Knowledge Graph | 三元组 `(entity) --relation--> (entity)` | 推理链完整 | 构建成本高，entity resolution 难，查询模式和 agent recall 不匹配 |
| C. Engram + Typed Links | flat memory + 带类型的 Hebbian links | 80% KG 能力，20% 复杂度 | 不支持深度图遍历推理 |

### 决策：方案 C — Engram + Causal/Typed Links

**理由：**
1. 已有知识和新知识不应该用不同系统。KB 一套 + engram 一套 = recall 查两处 + ranking 分裂 + 维护成本翻倍
2. Hebbian links 已经有 70% 基础，只需加 `link_type` 字段
3. Agent 的使用模式是"遇到问题→找经验"，不是"遍历图谱推理"
4. KG 的 entity resolution 很重（"CUDA OOM" vs "GPU out of memory" vs "RuntimeError: CUDA error" 是同一 entity？）

### 升级后的 Link Schema

```
现有 Hebbian Link:
{
  from_id, to_id,
  strength: 0.73,
  co_occurrence_count: 8
}

升级后:
{
  from_id, to_id,
  strength: 0.73,
  co_occurrence_count: 8,
  link_type: "solves",      // NEW: co_occurrence | causes | solves | contradicts | supersedes
  confidence: 0.9,          // NEW: 关系可信度
  source: "stackoverflow"   // NEW: 关系来源
}
```

### Link Types

| Type | 含义 | 例子 |
|------|------|------|
| `co_occurrence` | 经常一起出现（现有） | "tokio" 和 "async" 经常一起被 recall |
| `causes` | A 导致 B | "batch_size=64" causes "CUDA OOM" |
| `solves` | A 解决 B | "gradient checkpointing" solves "CUDA OOM" |
| `contradicts` | A 和 B 矛盾 | "use fp16" contradicts "fp16 causes NaN on this model" |
| `supersedes` | A 替代 B（B 过时） | "PyTorch 2.0 compile()" supersedes "manual fusion" |

### Seed Extraction 时同时输出关系

```
# LLM extraction 产出:
Memory A: "PyTorch CUDA OOM at batch_size=64"  (type: factual)
Memory B: "Reduce batch size to 32 or use gradient checkpointing"  (type: procedural)
Link: A --solves--> B  (confidence: 0.95, source: stackoverflow)
```

### Recall 增强

Query "CUDA OOM" → 找到 Memory A → 通过 `solves` link **自动拉出解决方案 B**。
不是随机共现，而是已知的因果/解决关系。

### Link Accuracy 问题与防线

**不准确的两层来源：**

1. **LLM 提取判断错误**
   - Correlation ≠ causation："我用了 X 然后 Y 没了" 不代表 X solves Y（可能同时做了其他改动）
   - `causes` 最容易错 — 时间先后 ≠ 因果
   - `contradicts` 最难判断 — 不同维度的 trade-off 不是矛盾

2. **领域模糊性**
   - `solves` vs `mitigates` — workaround 不等于 fix
   - `supersedes` 需要时间线知识，LLM 可能不知道哪个方案更新

**升级后的 Link Schema（含防线）：**

```
Link: A --solves--> B
  confidence: 0.7    ← LLM 自评
  source: "stackoverflow"
  verified: false    ← NEW: 人/agent 验证过没
```

**三层防线：**

| 层级 | 机制 | 效果 |
|------|------|------|
| ① 提取时 | LLM 自评 confidence | 明确因果("the fix is...")→0.9，隐含("after X, Y stopped")→0.6，模糊关联→0.3 降级为 co_occurrence |
| ② 使用时 | confidence 做 recall 权重 | recall 排序 = ACT-R activation × link_confidence。低 confidence typed link 效果 ≈ 普通 Hebbian |
| ③ 社区修正 | 用后反馈循环 | agent 用了 solves 建议没解决 → 自动降 confidence。多 agent 验证有效 → confidence 上升 |

**关键保证：confidence 低的 typed link 不比 co_occurrence 差。** Worst case 退化成普通 Hebbian co-occurrence（和没加 link_type 一样）。不会比现状更差，只可能更好。

**实现优先级：**
1. 先加 `confidence` 字段（必须有）[P0]
2. `link_type` 判断不确定时默认 `co_occurrence`（保守策略）[P0]
3. 只有 confidence > 0.7 的 typed link 才在 recall 时做因果链展开 [P0]
4. 后续加 `verified` 字段做社区修正 [P1]

### 待定
- 是否需要 `entity` 层？（多条记忆指向同一概念时做 dedup/merge）
- `supersedes` 如何自动触发？（检测到新版本知识时自动标记旧版本）
- Typed links 的 confidence 如何衰减？（和 memory importance 一样 ACT-R 衰减？还是固定？）
- Confidence 阈值 0.7 是否需要 per-domain 调整？

---

*This doc captures initial thinking. Next step: detailed requirements after discussion with potato.*
