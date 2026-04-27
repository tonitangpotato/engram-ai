# Engram (`engramai`) — 技术架构深度总结

> 生成时间: 2026-04-26 · 基于 `crates/engramai/src/` 源码

---

## 1. 核心数据结构 — `MemoryRecord`

**文件**: `src/types.rs` · `struct MemoryRecord`

| 字段 | 类型 | 认知对应 |
|---|---|---|
| `id` | String (8-char UUID) | 唯一标识 |
| `content` | String | 自然语言记忆内容 |
| `memory_type` | `MemoryType` enum | factual / episodic / relational / emotional / procedural / opinion / causal |
| `layer` | `MemoryLayer` enum | Core (新皮层) / Working (海马体) / Archive (冷存储) |
| `access_times` | `Vec<DateTime>` | ACT-R base-level activation 的关键输入 |
| `working_strength` | f64 | 海马体痕迹 (r₁), 快速衰减 |
| `core_strength` | f64 | 新皮层痕迹 (r₂), 慢速衰减 |
| `importance` | f64 (0–1) | 杏仁核调制因子 |
| `pinned` | bool | 永不衰减/永不删除 |
| `consolidation_count` | i32 | 经历过几次 "睡眠" 巩固 |
| `superseded_by` | Option\<String\> | 被哪条更新记忆取代 (filter-based, 召回时跳过) |
| `contradicted_by` | Option\<String\> | 矛盾链接 (penalty-based, 扣分) |
| `metadata` | Option\<JSON\> | 存 type_weights、用户自定义等结构化数据 |

每种 `MemoryType` 自带默认 `importance` 和 `decay_rate`（如 Emotional: importance=0.9, decay=0.01; Episodic: 0.4, 0.10）。

**Hebbian 链接** 是独立结构 `HebbianLink`（`src/types.rs`）：`source_id` ↔ `target_id` + `strength` + `coactivation_count` + `direction` + 可选跨 namespace 信息。

---

## 2. 存储流程 (Store)

**入口**: `Memory::store_raw()` @ `src/memory.rs:2252`

```
content + StorageMeta
    │
    ├─ Path A: 有 extractor (LLM) ──→ extractor.extract(content)
    │   ├─ 返回 Vec<ExtractedFact> ──→ 每个 fact → EnrichedMemory::from_extracted()
    │   │   → importance cap 到 config.auto_extract_importance_cap
    │   │   → store_enriched(em) → 写入 SQLite + embedding
    │   │   → 成功后 enqueue_pipeline_job() (v0.3 resolution pipeline)
    │   ├─ 返回空 facts ──→ Skipped { NoFactsExtracted }
    │   └─ 出错 ──→ Quarantined (持久化到 quarantine 表, 可 retry)
    │
    └─ Path B: 无 extractor ──→ EnrichedMemory::minimal(content)
        → 按 memory_type_hint 设 type_weights
        → store_enriched(em) → 同上
```

**关键细节**:
- **Dedup/Merge**: `store_enriched` 内部检查 embedding 相似度，高相似度时执行 merge（更新 access_times, 强化 strength）而非插入新记录，返回 `StoreOutcome::Merged`。
- **Quarantine**: 提取失败不会丢数据，写入 quarantine 表，支持 `retry_quarantined(max_items)` 重试，最多 5 次后标记 `permanently_rejected`。
- **WriteStats**: 每次写入产生 `StoreEvent`（Stored/Skipped/Quarantined），通过 `EventSink` trait 发出，内置 `CountingSink` 做统计。
- **新记忆初始化**: `working_strength = 1.0`, `core_strength = 0.0`, `layer = Working`。

---

## 3. 召回流程 (Recall) — 7 通道融合

**入口**: `Memory::recall_from_namespace()` @ `src/memory.rs:3014`

### 7 个信号通道

| # | 通道 | 归一化到 0–1 | 来源 |
|---|---|---|---|
| 1 | **FTS** | rank-based: `1 - rank/total` | SQLite FTS5 全文搜索 |
| 2 | **Embedding** | `(cosine_sim + 1) / 2` | Ollama nomic-embed-text |
| 3 | **ACT-R activation** | sigmoid 归一化 (见下) | `retrieval_activation()` |
| 4 | **Entity** | entity graph 匹配分 | 实体抽取 + 图搜索 |
| 5 | **Temporal** | 时间范围距离 | query 中解析的时间表达式 |
| 6 | **Hebbian** | graph connectivity | `hebbian_channel_scores()` |
| 7 | **Somatic** | 情感记忆偏置 | Damasio somatic marker |

### 融合公式

```
final_score = (Σ wᵢ × scoreᵢ) × affinity_multiplier
```

其中 `wᵢ` 是 **runtime 归一化**后的权重（`adj_w / Σadj_w`），由 config 基础权重 × **C7 adaptive query-type modifier** 算出。`query_classifier` 根据 query 类型（who/when/how/causal/emotional...）动态调整各通道权重。

`affinity_multiplier` = max(type_weights[i] × query_type_affinity[i])，让记忆自带的类型权重与查询意图做匹配。

### 后处理
1. **Confidence** 独立于 combined_score，由 embedding_sim + FTS命中 + entity_score + age_hours 计算
2. **min_confidence 过滤**
3. **Embedding dedup**（`recall_dedup_threshold` 余弦相似度去重，避免近义重复霸占结果）
4. **Hebbian co-activation 记录**: 被一起召回的记忆对增加 coactivation_count（为下次 recall 的 Hebbian 通道蓄力）
5. **Session working memory** (`ActiveContext`): 召回结果加入 session buffer（Miller's Law，限容量）

---

## 4. 认知科学机制

### 4.1 Ebbinghaus 遗忘曲线

**文件**: `src/models/ebbinghaus.rs`

```
R(t) = e^(-t / S)

S = base_S × spacing_factor × importance_factor × consolidation_factor
  = (1/decay_rate) × (1 + 0.5·ln(n_accesses+1)) × (0.5 + importance) × (1 + 0.2·consolidation_count)
```

- `base_S` 由 `MemoryType::default_decay_rate()` 取倒数（Emotional: 1/0.01=100天, Episodic: 1/0.10=10天）
- **间隔重复效应**: 每次访问通过 `ln(n+1)` 缓慢提升 stability
- `effective_strength(record) = (working_strength + core_strength) × R(t)` — 最终"这条记忆还活着吗"的评分
- `should_forget`: effective_strength < threshold 且未 pinned → 可以清除

### 4.2 ACT-R Activation

**文件**: `src/models/actr.rs`

```
A_i = B_i + Σ(W_j · S_ji) + importance_boost - contradiction_penalty

B_i = ln(Σ_k  t_k^(-d))     // d ≈ 0.5, t_k = 距第k次访问的秒数
```

- `B_i` 编码了 **频率 × 近因性** — 访问越频繁、越近，activation 越高
- `spreading_activation()`: 用 context keywords 的命中率做近似（简化版 ACT-R，不跑完整 chunk retrieval）
- **Sigmoid 归一化** (`normalize_activation`): `1 / (1 + e^(-(x - center)/scale))`, center=-5.5, scale=1.5。比旧的线性归一化 `(x+10)/20` 有 **3× 更好的近因性区分度**（有 test 验证）

### 4.3 Hebbian Learning

**文件**: `src/models/hebbian.rs`

- **触发**: `record_coactivation(storage, memory_ids, threshold)` — 当多条记忆在同一次 recall 中被返回
- **机制**: 每对记忆增加 `coactivation_count`；达到阈值后自动形成 `HebbianLink`
- **跨 namespace**: `record_cross_namespace_coactivation()` 处理不同 agent namespace 之间的关联
- **召回时提权**: Hebbian 通道作为第 6 通道参与 7-channel fusion，graph connectivity 转化为 0–1 分
- **完全涌现式**: 不依赖显式实体标注，纯从使用模式中产生关联网络

### 4.4 Consolidation（Memory Chain Model）

**文件**: `src/models/consolidation.rs` · 基于 Murre & Chessa 2011 双痕迹模型

```
dr₁/dt = -μ₁ · r₁(t)                      // 海马体痕迹，快速衰减
dr₂/dt = α · r₁(t) - μ₂ · r₂(t)          // 新皮层痕迹，从海马体转移，缓慢衰减
```

**`run_consolidation_cycle()` 四步（"睡眠"周期）**:
1. **Consolidate Working**: 对所有 Working 层记忆执行 `consolidate_single()` — 将 r₁ 转移到 r₂，importance² 调制 effective_alpha
2. **Interleaved Replay**: 随机采样 Archive 层记忆（比例 = `interleave_ratio`），微量 boost `core_strength`（防止灾难性遗忘）
3. **Decay Core**: Core 层记忆仅执行 μ₂ 慢衰减
4. **Rebalance Layers**: 基于 strength 阈值在 Working ↔ Core ↔ Archive 之间迁移

**层迁移规则**:
- Working → Core: `core_strength ≥ promote_threshold`
- Core → Archive: `total_strength < demote_threshold`
- Archive → Core: replay 够多后 `core_strength` 可回升

**`Memory::sleep()`** 是统一入口（`memory.rs`），串联 consolidation → synthesis → decay → forget → rebalance，产出 `SleepReport`。

---

## 5. Drive Alignment

**文件**: `src/bus/alignment.rs` · `score_alignment_hybrid()`

```rust
pub fn score_alignment_hybrid(content, drives, drive_embeddings, content_embedding) -> f64 {
    let keyword_score = score_alignment(content, drives);   // keyword 匹配
    let embedding_score = drive_embeddings.score(content_embedding);  // 语义相似度
    keyword_score.max(embedding_score)  // 取 max — 任一信号足够
}
```

- **`score_alignment()`**: 对每个 Drive 的 keywords 做词级匹配，`min(matches/3, 1.0)` 得分，多 drive 取均值
- **`calculate_importance_boost()`**: alignment score → `1.0 + (ALIGNMENT_BOOST - 1.0) × alignment` 线性插值
- 整合在 `EmpathyBus`（`src/bus/mod.rs`）中：存储时自动算 drive alignment → 调 importance boost；也可手动查询 `score_content()`

**Drive** 来自 SOUL.md 解析（`bus/mod.rs`），包含 name + description + keywords。`DriveEmbeddings` 预计算所有 drive 的 embedding 向量，recall 时免重算。

---

## 6. 关键设计决策 & Trade-offs

### 架构选择

| 决策 | 理由 |
|---|---|
| **SQLite 单文件** | 零依赖部署，agent 随身携带记忆；FTS5 + 自建 embedding 列 |
| **7 通道 runtime 归一化** | 避免任何单通道垄断；权重和恒为 1.0，新通道加入不破坏旧行为 |
| **Sigmoid vs Linear activation norm** | 旧线性压缩有效区间到 0.13–0.40，sigmoid 给出 3× 近因性区分度 |
| **Quarantine 而非丢弃** | LLM extractor 不可靠，失败内容持久化 + 可重试（最多 5 次） |
| **Supersession > Contradiction** | 矛盾用 penalty 扣分（软），取代用 filter 排除（硬）；新信息完全替代旧信息 |
| **涌现式 Hebbian > 显式图谱** | 不依赖 entity extraction 质量，纯从共召回模式中建关联 |
| **Dual-trace consolidation** | Working (fast decay μ₁≈0.1) + Core (slow decay μ₂≈0.005)，importance² 调制转移速率 |
| **Config presets** | `MemoryConfig::chatbot()` / `task_agent()` / `personal_assistant()` / `researcher()` — 不同场景不同衰减/巩固参数 |

### v0.3 新增

- **Resolution Pipeline**: `store_raw` 插入后异步 enqueue `pipeline_job`（triple extraction、graph building）
- **Synthesis Engine**: sleep cycle 内可选 LLM 合成洞察（clustering → gating → provenance tracking → undo）
- **ActiveContext** (原 SessionWorkingMemory): session 级别的 Miller's Law buffer，与 L2 working_strength 区分
- **MetaCognition Tracker**: 自省 recall/synthesis 事件，产出参数调优建议

---

## 文件速查表

| 文件 | 职责 |
|---|---|
| `src/types.rs` | MemoryRecord, MemoryType, HebbianLink, RecallResult 等核心类型 |
| `src/memory.rs` | Memory 主 API: store_raw, recall, sleep, consolidate |
| `src/models/actr.rs` | ACT-R base-level activation + sigmoid normalization |
| `src/models/ebbinghaus.rs` | 遗忘曲线 R(t), stability S, effective_strength |
| `src/models/hebbian.rs` | co-activation 记录 + 跨 namespace 关联 |
| `src/models/consolidation.rs` | 双痕迹衰减 + 巩固周期 + 层迁移 |
| `src/bus/alignment.rs` | score_alignment_hybrid, calculate_importance_boost |
| `src/bus/mod.rs` | EmpathyBus, Drive, DriveEmbeddings |
| `src/store_api.rs` | StorageMeta, StoreOutcome, Quarantine types |
| `src/enriched.rs` | EnrichedMemory — extractor 输出的结构化中间表示 |
| `src/hybrid_search.rs` | 独立 hybrid_search() (vector + FTS RRF) |
| `src/query_classifier.rs` | C7 自适应权重 — regex L1 + Haiku L2 意图分类 |
| `src/config.rs` | MemoryConfig + presets |
| `src/lifecycle.rs` | decay/forget/rebalance 生命周期管理 |
| `src/synthesis/` | LLM 合成洞察 (clustering, gating, provenance) |
| `src/session_wm.rs` | ActiveContext — session 级活跃记忆 buffer |
