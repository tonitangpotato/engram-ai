# Design: Dimensional Memory Extraction

**Date**: 2026-04-21
**Requirements**: `.gid/features/dimensional-extract/requirements.md`
**Status**: Draft

---

## 1. Overview

将 engram 的 extract 层从"单字符串压缩"升级为"多维度结构化抽取"。三个核心变更：

1. **ExtractedFact 扩展** — 从单一 `content` 字段变为 11 个语义维度字段（含 sentiment/stance）
2. **TypeWeights 替代 MemoryType 单选** — 7 种 type 各有 0-1 权重，由维度填充规则推导
3. **Raw 层保底** — 原始文本存入 metadata，不丢弃

设计原则：**1 次 LLM call，structured output，规则式 type 推导，零额外 recall 开销**。

---

## 2. Architecture

数据流（改动部分加 `*`）：

```
raw text
  → *MemoryExtractor::extract()    [structured output, 11 维度]
  → *ExtractedFact (dimensional)
  → *type_weights = infer_type_weights(&fact)  [规则式]
  → *add_raw() stores: content=core_fact, metadata={dimensions + source_text + type_weights}
  → embedding on content (不变)
  → recall() → *type_affinity × type_weights (连续乘法)
```

关键决策：
- **维度存 metadata JSON，不加列** — 避免 schema 膨胀，维度是可变的（未来可能加新维度），JSON 灵活
- **content 字段 = 核心事实维度** — 保持 embedding/FTS/dedup 全部基于 content，零改动
- **type_weights 存 metadata** — recall 时从 metadata 解析，旧记忆 metadata 无此字段则用默认权重

---

## 3. Components

### 3.1 ExtractedFact 扩展

**文件**: `src/extractor.rs`

现有：
```rust
pub struct ExtractedFact {
    pub content: String,
    pub memory_type: MemoryType,
    pub importance: f64,
    pub tags: Vec<String>,
}
```

改为：
```rust
pub struct ExtractedFact {
    /// 核心事实（必填）— 直接映射到 MemoryRecord.content
    pub core_fact: String,
    /// 参与者
    pub participants: Option<String>,
    /// 时间信息
    pub temporal: Option<String>,
    /// 地点/来源
    pub location: Option<String>,
    /// 背景/情境
    pub context: Option<String>,
    /// 原因/动机
    pub causation: Option<String>,
    /// 结果/影响
    pub outcome: Option<String>,
    /// 方法/步骤
    pub method: Option<String>,
    /// 关系（与其他已知事物的关联）
    pub relations: Option<String>,
    /// 情感表达（如果有）
    pub sentiment: Option<String>,
    /// 立场/偏好/观点（如果有）
    pub stance: Option<String>,
    /// 重要性（LLM 判断）
    pub importance: f64,
    /// 标签
    pub tags: Vec<String>,
    /// 置信度: "confident" / "likely" / "uncertain"
    #[serde(default = "default_confidence")]
    pub confidence: String,
    /// 情感极性: -1.0 (very negative) to 1.0 (very positive). 0.0 = neutral.
    /// 驱动 interoceptive emotion system (last_extraction_emotions cache).
    #[serde(default)]
    pub valence: f64,
    /// 领域: "coding" / "trading" / "research" / "communication" / "general"
    /// 用于 interoceptive emotion routing.
    #[serde(default = "default_domain")]
    pub domain: String,
}
```

变更说明：
- `content` → `core_fact`（语义更清晰，且避免与 MemoryRecord.content 混淆）
- 移除 `memory_type` — 不再要求 LLM 选择，由规则推导
- 新增 10 个 `Option<String>` 维度字段（8 原始维度 + sentiment + stance）
- 保留 `confidence`、`valence`、`domain` 字段 — 这 3 个是 interoceptive emotion 系统的输入源，删除会 break 情感追踪
- 这是 breaking change，0.x semver minor bump 处理

### 3.2 Structured Output Prompt

**文件**: `src/extractor.rs`（`DefaultMemoryExtractor::extract` 方法）

现有 prompt 返回 JSON array `[{content, memory_type, importance, tags}]`。

改为 structured output schema：

```json
{
  "type": "object",
  "properties": {
    "memories": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "core_fact":     {"type": "string", "description": "What happened — the essential fact"},
          "participants":  {"type": "string", "description": "Who was involved"},
          "temporal":      {"type": "string", "description": "When it happened"},
          "location":      {"type": "string", "description": "Where / in what context"},
          "context":       {"type": "string", "description": "Background / surrounding situation"},
          "causation":     {"type": "string", "description": "Why it happened / motivation"},
          "outcome":       {"type": "string", "description": "What resulted / impact"},
          "method":        {"type": "string", "description": "How it was done / steps"},
          "relations":     {"type": "string", "description": "Connections to other known things"},
          "sentiment":     {"type": "string", "description": "Emotional expression if present (e.g. frustrated, excited, relieved)"},
          "stance":        {"type": "string", "description": "Opinion / preference / position if present (e.g. prefers X over Y, believes Z)"},
          "importance":    {"type": "number", "description": "0.0-1.0 importance score"},
          "tags":          {"type": "array", "items": {"type": "string"}},
          "confidence":    {"type": "string", "description": "confident (direct statement), likely (reasonable inference), uncertain (vague mention)"},
          "valence":       {"type": "number", "description": "-1.0 (very negative) to 1.0 (very positive). 0.0 = neutral. Consider speaker's emotional state, not just keywords."},
          "domain":        {"type": "string", "description": "Which domain: coding, trading, research, communication, general"}
        },
        "required": ["core_fact", "importance", "tags", "confidence", "valence", "domain"]
      }
    }
  },
  "required": ["memories"]
}
```

实现方式：
- **`MemoryExtractor` trait 不变** — `async fn extract(&self, text: &str) -> Result<Vec<ExtractedFact>>`
- **`DefaultMemoryExtractor`** 改用 tool_use / function_calling 发送 schema
- 通过 `ExtractionConfig` 上的 provider hint 决定用 Anthropic tool_use 还是 OpenAI function_calling
- **Fallback**：如果 provider 不支持 structured output，走现有 prompt-based JSON 解析路径（使用**旧 prompt**，含 memory_type 字段），产出走 legacy parser path（§3.6 Path 2），维度全空，type_weights 走默认值。行为等价于"改动前"，满足 GUARD-2。

Prompt 指引（system message 部分）：
```
Extract memories from the following conversation. For each distinct memory:
- core_fact: The essential information (required)
- Fill other dimensions ONLY if the information is explicitly present
- Do NOT infer or fabricate — leave dimensions empty rather than guess
- importance: 0.0-1.0 based on long-term relevance
```

### 3.3 TypeWeights 推导

**文件**: 新建 `src/type_weights.rs`

```rust
/// 7 种 memory type 的连续权重（0.0 - 1.0）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeWeights {
    pub factual: f64,
    pub episodic: f64,
    pub procedural: f64,
    pub relational: f64,
    pub emotional: f64,
    pub opinion: f64,
    pub causal: f64,
}

impl Default for TypeWeights {
    fn default() -> Self {
        // 旧记忆的默认权重：全 1.0，等价于“所有 type 等概率”
        // 这样旧记忆在 recall 时 type_boost = max(1.0 * affinity_i) = max(affinity_i)
        // 行为与改动前完全一致（旧逻辑是精确匹配一个 type 拿到该 type 的 affinity）
        Self {
            factual: 1.0, episodic: 1.0, procedural: 1.0,
            relational: 1.0, emotional: 1.0, opinion: 1.0, causal: 1.0,
        }
    }
}
```

**推导规则** `fn infer_type_weights(fact: &ExtractedFact) -> TypeWeights`：

| 条件 | 权重变化 |
|---|---|
| `core_fact` 非空 | factual += 0.4 |
| `temporal` 非空 | episodic += 0.5 |
| `participants` 非空 | relational += 0.4, episodic += 0.2 |
| `causation` 非空 | causal += 0.5 |
| `outcome` 非空 | causal += 0.3 |
| `method` 非空 | procedural += 0.5 |
| `context` 非空 | episodic += 0.2 |
| `location` 非空 | episodic += 0.1 |
| `relations` 非空 | relational += 0.3 |
| `sentiment` 非空 | emotional += 0.5 |
| `stance` 非空 | opinion += 0.5 |
| 基线 | 所有 type 起始 0.1 |

规则叠加后 clamp 到 [0.0, 1.0]。

**emotional / opinion 的处理**：这两个 type 无法从其他维度有无推导（"potato prefers Rust" 维度上看只有 core_fact + participants，但它是 opinion）。方案：

- 直接作为 LLM 输出的维度字段：`sentiment`（情感表达）和 `stance`（立场/观点）
- LLM 本身就在理解上下文做 extract，顺便判断有无情感/观点比任何规则都准
- 零额外 LLM call — 只是 structured output 多两个 optional 字段
- 推导规则统一：`sentiment` 非空 → emotional += 0.5，`stance` 非空 → opinion += 0.5

### 3.4 Storage Integration

**文件**: `src/memory.rs`（`add_raw` 方法）

#### 存储路径

`Memory::add()` 中 extract 成功后，对每个 `ExtractedFact`：

1. `content` = `fact.core_fact`（直接映射，embedding 基于此）
2. 计算 `type_weights = infer_type_weights(&fact)`
3. 构建 metadata JSON：

```json
{
  "dimensions": {
    "participants": "potato",
    "temporal": "2026-04-21",
    "causation": "Python too slow for the workload",
    "outcome": "Decided to rewrite in Rust"
  },
  "type_weights": {
    "factual": 0.5, "episodic": 0.7, "procedural": 0.1,
    "relational": 0.4, "emotional": 0.1, "opinion": 0.1, "causal": 0.8
  },
  "source_text": "昨天 potato 因为 Python 太慢决定换 Rust..."
}
```

4. `memory_type` 字段（DB 列仍需填写）：取 `type_weights` 中最高权重的 type 作为 primary type。注意：`add()` / `add_to_namespace()` 的 `memory_type: MemoryType` 参数签名不变（GOAL-4 兼容），但在 extract 成功时该参数**被忽略**（改为 type_weights 推导），仅在 extract 失败 fallback 时使用。
5. `source_text` 附加到**每条** extracted fact 上（避免 dedup 合并删除 fact[0] 时丢失原文，磁盘开销可忽略）
6. 如果调用者已传入 metadata，与 dimensional metadata **merge**（调用者的 key 优先）

#### Schema Migration

不需要新增列。维度数据、type_weights、source_text 全部存入现有 `metadata TEXT` 列的 JSON。

旧记忆的 metadata 可能是 null 或不含这些 key — recall 时缺失则用默认值。

#### Migration 兼容

唯一需要的 migration：无。现有 schema 完全兼容。

旧记忆在 recall 时：
- `content` → 当作 core_fact，其余维度为空
- `metadata` 无 `type_weights` → 用 `TypeWeights::default()`（全 1.0）
- `metadata` 无 `source_text` → 不可恢复原文（这是旧数据的固有限制）

### 3.5 Recall Integration

**文件**: `src/memory.rs`（`recall` 方法，scoring 部分）

现有逻辑（`memory.rs:1936` 附近）：

```rust
// 当前：离散匹配
let type_boost = match record.memory_type {
    MemoryType::Factual => query_analysis.type_affinity.factual,
    MemoryType::Episodic => query_analysis.type_affinity.episodic,
    // ... 7 个 match arm
};
score *= type_boost;
```

改为 **加权最大值** 策略：

```rust
let type_weights = TypeWeights::from_metadata(&record.metadata);
let affinity = &query_analysis.type_affinity;
let type_boost = [
    type_weights.factual    * affinity.factual,
    type_weights.episodic   * affinity.episodic,
    type_weights.procedural * affinity.procedural,
    type_weights.relational * affinity.relational,
    type_weights.emotional  * affinity.emotional,
    type_weights.opinion    * affinity.opinion,
    type_weights.causal     * affinity.causal,
]
.iter()
.cloned()
.fold(f64::NEG_INFINITY, f64::max);
score *= type_boost;
```

**为什么用 max 而不是 mean**：旧逻辑 type_boost 是单个 affinity 值（典型范围 0.3-2.5）。mean 会把强匹配稀释掉（causal=0.9×2.5=2.25 被其他 6 个弱维度拉到 0.53），而 max 保留了"最强维度匹配"的语义（2.25），与旧逻辑行为一致。

**旧记忆行为**：`TypeWeights::default()` 全 1.0 → `type_boost = max(1.0 * affinity_i) = max(affinity_i)`。对于 neutral affinity（全 1.0），type_boost = 1.0 — 与改动前完全一致。

**新记忆行为**：维度推导产出的 weights 范围 0.1-1.0，max 策略下：
- 强匹配场景（causal 记忆 + causal 查询）：`type_boost = 0.9 × 2.5 = 2.25`（接近旧逻辑的 2.5，但更精确——0.9 反映"几乎是 causal"而非"100% causal"）
- 弱匹配场景（factual 记忆 + causal 查询）：`type_boost = max(0.5×0.8, 0.1×2.5, ...) = 0.40`（被降权，正确行为）
- Neutral 查询（General intent, affinity 全 1.0）：`type_boost = max(weight_i × 1.0) = max(weight_i)`。新记忆最高 weight 通常 0.5-0.9，所以 type_boost < 1.0，意味着新记忆在 neutral 查询下比旧记忆略低。**这是可接受的** — 新记忆有更精确的 type 信号，在 targeted 查询中获得更大 boost，在 neutral 查询中略有代价，是 precision-recall 的合理 trade-off。

### 3.6 Parsing & Deserialization

**文件**: `src/extractor.rs`（`parse_extraction_response` 函数）

现有 `parse_extraction_response` 解析 JSON `[{content, memory_type, importance, tags}]`。

改为 dual-path parser：

```rust
fn parse_extraction_response(content: &str) -> Result<Vec<ExtractedFact>> {
    let json_str = strip_markdown_fences(content);
    
    // Path 1: 尝试新格式 {"memories": [{core_fact, ...}]}
    if let Ok(dimensional) = serde_json::from_str::<DimensionalResponse>(json_str) {
        return Ok(dimensional.memories.into_iter().map(Into::into).collect());
    }
    
    // Path 2: 兼容旧格式 [{content, memory_type, importance, tags}]
    if let Ok(legacy) = serde_json::from_str::<Vec<LegacyExtractedFact>>(json_str) {
        return Ok(legacy.into_iter().map(|f| ExtractedFact {
            core_fact: f.content,
            importance: f.importance,
            tags: f.tags,
            // 所有维度为 None — 旧格式没有这些字段
            ..Default::default()
        }).collect());
    }
    
    Err("Failed to parse extraction response".into())
}
```

`LegacyExtractedFact` 是旧 `ExtractedFact` 的私有副本，仅用于解析兼容。

---

## 4. Data Flow

### 4.1 Extract → Store

```
Input: "昨天 potato 因为 Python 太慢决定换 Rust，花了一晚上重写核心模块"

LLM structured output:
{
  "memories": [{
    "core_fact": "potato 决定从 Python 换到 Rust 并重写了核心模块",
    "participants": "potato",
    "temporal": "昨天（2026-04-20）",
    "causation": "Python 性能不足",
    "outcome": "核心模块用 Rust 重写完成",
    "method": "花一晚上重写",
    "importance": 0.7,
    "tags": ["rust", "python", "rewrite", "performance"]
  }]
}

infer_type_weights → {
  factual: 0.5,     // core_fact 非空 (0.1 + 0.4)
  episodic: 0.8,    // temporal (0.1 + 0.5) + participants (0.2)
  procedural: 0.6,  // method (0.1 + 0.5)
  relational: 0.5,  // participants (0.1 + 0.4)
  emotional: 0.1,   // sentiment 为空
  opinion: 0.1,     // stance 为空
  causal: 0.9,      // causation (0.1 + 0.5) + outcome (0.3)
}

Stored as:
  content = "potato 决定从 Python 换到 Rust 并重写了核心模块"
  memory_type = Causal  (highest weight)
  metadata = {dimensions: {...}, type_weights: {...}, source_text: "昨天 potato 因为..."}
```

### 4.2 Recall Scoring

```
Query: "potato 为什么换 Rust？"
QueryIntent: Causal → type_affinity = {factual: 0.8, episodic: 0.5, procedural: 0.3, relational: 0.8, emotional: 0.3, opinion: 0.5, causal: 2.5}

For the above memory (type_weights from §4.1):
type_boost = max(
    0.5*0.8,   // factual:    0.40
    0.8*0.5,   // episodic:   0.40
    0.6*0.3,   // procedural: 0.18
    0.5*0.8,   // relational: 0.40
    0.1*0.3,   // emotional:  0.03
    0.1*0.5,   // opinion:    0.05
    0.9*2.5,   // causal:     2.25  ← winner
) = 2.25

Compare with old behavior (single Causal type):
type_boost = 2.5 (direct match)

新逻辑 2.25 vs 旧逻辑 2.5 — 差异来自 causal weight 是 0.9 而非 1.0。
这是 feature: 记忆不是 100% causal（还有 episodic/procedural 成分），所以 boost 略低。
```

**旧记忆对比**（TypeWeights 全 1.0）：`type_boost = max(1.0 × affinity_i) = max(2.5) = 2.5` — 与改动前完全一致。

---

## 5. Guard Verification

| Guard | 如何满足 |
|---|---|
| GUARD-1 (成本可控) | 1 次 LLM call，structured output schema 替代自由格式，token 增量 ~10-15%（JSON key 开销） |
| GUARD-2 (无信息退化) | core_fact ≥ 旧 content；额外维度只增不减；raw 层保底 |
| GUARD-3 (Recall 性能) | 7 次乘法 + 1 次 max，O(1)，可忽略 |
| GUARD-4 (Migration 安全) | 无 schema 变更，维度存 metadata JSON；旧记忆用默认权重 |
| GUARD-5 (Dedup 兼容) | content = core_fact，embedding 基于此，dedup 逻辑不变。但 core_fact 可能比旧 content 更短更精炼 — 需要实测验证阈值 |

---

## 6. Implementation Plan

1. **新建 `src/type_weights.rs`** — TypeWeights struct + `infer_type_weights()` + Default impl
2. **改 `src/extractor.rs`** — ExtractedFact 新字段（含 confidence/valence/domain） + structured output prompt + dual-path parser (§3.6)
3. **改 `src/memory.rs`** — 主要改动在 `add_to_namespace()` 的 fact 循环内：构建 dimensional metadata、type_weights 推导、source_text 附加。`add_raw()` 可能不需要改（metadata 已由上层构建好传入）。`recall()` 中替换 type_boost 逻辑为 max 策略。
4. **测试** — type_weights 推导单元测试、extract 解析测试（新格式 + legacy fallback）、recall scoring 测试、旧记忆兼容测试
5. **Benchmark** — 50 条现有记忆，对比新旧 recall 排序

---

## 7. Open Questions

1. **Dedup 阈值调整** — core_fact 比旧 content 更短，embedding 距离可能变化。需要实测数据再决定是否调阈值。
2. **Sentiment/Stance 粒度** — 当前是 free-text string，未来可能需要结构化（如 valence float + label enum），但 v1 先用自由文本让 LLM 自行表达。
