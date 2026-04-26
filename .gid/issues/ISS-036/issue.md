---
id: "ISS-036"
title: "Synthesis Engine O(n²) Performance Bottleneck"
status: open
priority: P2
created: 2026-04-18
---
# ISS-036: Synthesis Engine O(n²) Performance Bottleneck

**Status:** Open  
**Priority:** High  
**Created:** 2026-04-18  
**Affects:** `clustering.rs`, `synthesis/cluster.rs`, `storage.rs`

---

## Problem

N2N (全量记忆) synthesis 在 13,085 条记忆上耗时 443 秒。两个独立瓶颈：

### Bottleneck 1: Per-ID Hebbian/Entity Loading

`synthesis/cluster.rs` 的 `discover_clusters()` Step 2 为每条候选记忆单独查 DB（查询和 node 构建已分成两步，但查询本身仍是 per-ID 循环）：

```rust
// Step 2: Build signal maps (pre-compute for efficient node construction)

// Hebbian links — N 次 per-ID SQL 查询
let mut hebbian_map: HashMap<String, Vec<(String, f64)>> = HashMap::new();
for m in &candidates {
    if let Ok(links) = storage.get_hebbian_links_weighted(&m.id) {  // 13k 次
        for (neighbor, weight) in links {
            if candidate_ids.contains(neighbor.as_str()) {
                hebbian_map.entry(m.id.clone()).or_default().push((neighbor, weight));
            }
        }
    }
}

// Entity IDs per memory — N 次 per-ID SQL 查询
let mut entity_map: HashMap<String, Vec<String>> = HashMap::new();
for m in &candidates {
    if let Ok(entities) = storage.get_entity_ids_for_memory(&m.id) {  // 13k 次
        entity_map.insert(m.id.clone(), entities);
    }
}
```

13,085 × 2 = 26,170 次 SQLite 查询。每次查询有 prepare/bind/fetch 开销。
实际数据：全库只有 264 条 Hebbian links。26k 次查询绝大多数返回空。

### Bottleneck 2: O(n²) k-NN Graph Construction

`clustering.rs` 的 `InfomapClusterer::cluster()` Step 1 构建 k-NN 图：

```rust
for i in 0..nodes.len() {
    for j in (i + 1)..nodes.len() {
        let w = self.strategy.compute_weight(&nodes[i], &nodes[j]);
        // ...
    }
}
```

13,085² / 2 = 85,609,060 次 `compute_weight()` 调用。
`MultiSignal::compute_weight` 涉及：embedding cosine + Hebbian lookup + entity Jaccard + time proximity。
即使每次 1µs，仍需 ~86 秒。

---

## Root Cause Analysis

这不是"优化"问题，是架构缺陷：

1. **数据加载层缺少 bulk API。** `storage.rs` 只提供 per-ID 查询接口，没有全量加载方法。`synthesis/cluster.rs` 被迫逐条查询。
2. **k-NN 图构建没有候选剪枝。** 四种信号中 embedding cosine 是计算量最大的，但也是唯一可以用 ANN (Approximate Nearest Neighbor) 索引加速的。当前实现对所有 n(n-1)/2 对都计算全部四种信号。

两个问题是独立的。修 #1 不需要改 #2，反之亦然。但都需要修。

---

## Solution Design

### Fix 1: Bulk Data Loading (storage.rs + synthesis/cluster.rs)

#### 新增 `storage.rs` 方法

```rust
/// Load ALL Hebbian links into memory as bidirectional adjacency map.
/// Single SQL query replaces 13k per-ID queries.
pub fn get_all_hebbian_links_bulk(&self) -> Result<HashMap<String, Vec<(String, f64)>>, rusqlite::Error> {
    // SELECT source_id, target_id, strength FROM hebbian_links WHERE strength > 0
    // Index by both source_id → (target_id, strength) and target_id → (source_id, strength)
}

/// Load ALL memory-entity associations into memory.
/// Single SQL query replaces 13k per-ID queries.
pub fn get_all_memory_entities_bulk(&self) -> Result<HashMap<String, Vec<String>>, rusqlite::Error> {
    // SELECT memory_id, entity_id FROM memory_entities
    // Index by memory_id → [entity_id, ...]
}
```

#### 修改 `synthesis/cluster.rs` Step 2

```rust
// Before (Step 2 — per-ID queries):
let mut hebbian_map: HashMap<String, Vec<(String, f64)>> = HashMap::new();
for m in &candidates {
    if let Ok(links) = storage.get_hebbian_links_weighted(&m.id) {
        for (neighbor, weight) in links {
            if candidate_ids.contains(neighbor.as_str()) {
                hebbian_map.entry(m.id.clone()).or_default().push((neighbor, weight));
            }
        }
    }
}
let mut entity_map: HashMap<String, Vec<String>> = HashMap::new();
for m in &candidates {
    if let Ok(entities) = storage.get_entity_ids_for_memory(&m.id) {
        entity_map.insert(m.id.clone(), entities);
    }
}

// After (2 bulk queries + candidate filtering):
let all_hebbian = storage.get_all_hebbian_links_bulk()?;
let all_entities = storage.get_all_memory_entities_bulk()?;
// Filter to candidate set only (preserve existing behavior)
let hebbian_map: HashMap<String, Vec<(String, f64)>> = all_hebbian
    .into_iter()
    .filter(|(id, _)| candidate_ids.contains(id.as_str()))
    .map(|(id, links)| {
        let filtered = links.into_iter()
            .filter(|(neighbor, _)| candidate_ids.contains(neighbor.as_str()))
            .collect();
        (id, filtered)
    })
    .collect();
let entity_map: HashMap<String, Vec<String>> = all_entities
    .into_iter()
    .filter(|(id, _)| candidate_ids.contains(id.as_str()))
    .collect();
// Step 3 (ClusterNode construction from maps) remains unchanged.
```

**复杂度：** 26,170 SQL queries → 2 SQL queries。
**估计改进：** 443s 中大部分 I/O 开销消除，预期 data loading 降到 <1s。

### Fix 2: Two-Stage k-NN Graph (clustering.rs)

核心思路：embedding 是唯一支持 ANN 索引的信号。用 embedding ANN 做候选剪枝（stage 1），再对候选对计算完整 MultiSignal 权重（stage 2）。

#### Stage 1: VP-Tree Candidate Pruning

VP-tree（Vantage-Point Tree）用于高维距离空间的近邻搜索。相比 kd-tree：
- kd-tree 在 dim > 20 时退化到线性扫描
- VP-tree 对任意 metric space 有效，包括高维 cosine distance
- 实现简单（~80 行 Rust），无外部依赖

```
构建: O(n log n)
查询: O(log n) 平均, O(n) 最坏
总计: O(n log n) + O(n × log n) = O(n log n)
```

每个节点查 top-k' 个 embedding 近邻（k' > k，取 k 的 2~3 倍作为 over-fetch factor，保证 MultiSignal 重排后 top-k 质量）。

#### Stage 2: Hebbian + Entity Pair Injection + MultiSignal Reranking

Hebbian links 是先验知识 —— 如果两条记忆有 Hebbian 关联，它们一定是候选对，不需要 ANN 发现。

Entity overlap 也是独立于 embedding 的信号 —— 两条记忆可能 embedding 距离远但共享大量 entity（比如都提到 "RustClaw, engram"），这类 entity-driven edges 会被纯 embedding ANN 漏掉。

```
candidate_pairs = ANN_top_k'(embedding)
                ∪ hebbian_linked_pairs
                ∪ entity_co_occurring_pairs
```

**Entity co-occurring pairs** 从 bulk entity 数据构建反向索引（entity_id → [memory_ids]），对每个 entity 的 memory 列表取两两配对。由于大多数 entity 只关联少量记忆（<10），这个集合通常比 ANN 候选集小得多。

如果 entity 数据太稀疏（profile 显示 co-occurring pairs < 1% of ANN pairs），可以跳过此注入但需记录为 known gap。

对 candidate_pairs 调 `strategy.compute_weight()`，取 top-k 加入 Infomap。

#### 不改的接口

- `EdgeWeightStrategy` trait — 完全不动
- `MultiSignal::compute_weight` — 完全不动
- `ClusterNode` struct — 完全不动
- `InfomapClusterer::cluster()` 签名和返回值 — 不变
- 当节点没有 embedding 时 — fallback 到 O(n²) brute force

#### Scope: Both Callers Benefit

VP-tree two-stage 实现在 `InfomapClusterer::cluster()` 内部，所以两条调用路径都自动受益：
- `synthesis/cluster.rs` → `InfomapClusterer<MultiSignal>` — 主要瓶颈，4 信号
- `compiler/discovery.rs` → `InfomapClusterer<EmbeddingOnly>` — 同样面对 13k+ 节点的 O(n²)，且 ANN 精度最高（只有 embedding 一个信号，不需要任何注入策略）

#### VP-Tree 实现

内嵌在 `clustering.rs` 中，不做独立模块（只被 k-NN 图构建使用）。

```rust
struct VpTree {
    nodes: Vec<VpNode>,
}

struct VpNode {
    index: usize,       // index into original embeddings array
    threshold: f32,     // median distance to children
    left: Option<usize>,  // subtree within threshold
    right: Option<usize>, // subtree beyond threshold
}

impl VpTree {
    fn build(embeddings: &[&[f32]], distance_fn: fn(&[f32], &[f32]) -> f32) -> Self { ... }
    fn query_nearest(&self, query: &[f32], k: usize, embeddings: &[&[f32]]) -> Vec<(usize, f32)> { ... }
}
```

**VP-tree query pruning 逻辑（关键正确性）：**
```
fn search(node, query, k, results):
    d = distance(query, embeddings[node.index])
    // update results (max-heap of k nearest, keyed by distance)
    if results.len() < k || d < results.peek().distance:
        results.push((node.index, d))
        if results.len() > k: results.pop()  // remove farthest
    
    tau = results.peek().distance  // current k-th nearest distance (or ∞)
    
    if d < node.threshold:
        // query is inside the ball → search left first (likely closer)
        if d - tau < node.threshold:  // left subtree may contain nearer points
            search(node.left, ...)
        if d + tau >= node.threshold:  // right subtree may also contain nearer points
            search(node.right, ...)
    else:
        // query is outside the ball → search right first
        if d + tau >= node.threshold:
            search(node.right, ...)
        if d - tau < node.threshold:
            search(node.left, ...)
```

NaN 安全：距离比较用 `partial_cmp(...).unwrap_or(Ordering::Greater)`（当前代码风格，无需 `ordered-float` crate）。

距离函数用 **Euclidean distance on L2-normalized vectors**：`||a - b|| = sqrt(2(1 - cos(a,b)))`。

⚠️ **不能用 cosine distance (1 - cosine_sim)** — 它不满足三角不等式，VP-tree 剪枝正确性依赖三角不等式，用 cosine distance 会漏掉真正的近邻。Euclidean distance 在归一化向量上与 cosine distance 单调等价，但满足 metric 条件。

实现时在 VP-tree 构建前 assert L2 norm ≈ 1.0（nomic-embed-text 输出通常是归一化的，但需要运行时验证）：
```rust
// Pre-check: verify embeddings are L2-normalized
for (id, emb) in &embeddings {
    let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
    debug_assert!((norm - 1.0).abs() < 0.01, "embedding {id} not L2-normalized: norm={norm}");
}
```

**复杂度：**
- 当前: O(n²) × 4 signals
- 改后: O(n log n) build + O(n × k' × log n) query + O(n × k' × 4 signals) rerank
- n=13k, k'=45 (3k, k=15): 13k × 45 = 585k 次 compute_weight，vs 之前 85M 次 → **~146x 加速**

---

## What Does NOT Change

| Component | Changes? | Reason |
|---|---|---|
| `EdgeWeightStrategy` trait | ❌ No | ANN 剪枝对 strategy 透明 |
| `MultiSignal::compute_weight` | ❌ No | 只是被调用次数减少 |
| `EmbeddingOnly` strategy | ❌ No | 也受益于 VP-tree（though it's simpler） |
| Infomap 调用 | ❌ No | 输入是 edge list，不关心怎么生成 |
| `ClusterNode` struct | ❌ No | 字段不变 |
| `discover_clusters` 签名 | ❌ No | 返回值不变 |
| 所有现有 tests | ❌ Must pass | 行为一致性 |
| Cohesion O(m²) (`clustering.rs:339`) | ❌ No | Per-cluster, m≤50, ~245k ops total — not a bottleneck |
| `find_centroid` O(m²) (`synthesis/cluster.rs:213`) | ❌ No | Same: per-cluster, small m. Hebbian lookups remain valid (data in ClusterNode fields, not external map) |

---

## Testing Strategy

### Unit Tests (新增)

1. **VP-tree 基础**
   - 构建空集 → 不 panic
   - 单点 → query 返回自己
   - 10 个点 → query top-3 与 brute-force 结果一致
   - 高维（768 维）→ 结果正确

2. **Two-stage vs brute-force 一致性**
   - 小数据集（20 nodes）：two-stage 与 O(n²) 产生相同的 Infomap 输入边
   - 验证 Hebbian 注入：Hebbian-linked pair 即使 embedding 距离远，仍出现在候选集

3. **Bulk 方法**
   - `get_all_hebbian_links_bulk` 返回与逐条查询相同的结果
   - `get_all_memory_entities_bulk` 返回与逐条查询相同的结果
   - 空表 → 返回空 HashMap

### 回归 (现有 tests)

所有 708 个现有 test 必须通过，0 个失败。

---

## Estimated Impact

| Metric | Before | After | Improvement |
|---|---|---|---|
| SQL queries (data load) | 26,170 | 2 | 13,085x |
| compute_weight calls | 85,609,060 | ~585,000 (k'=3k=45) | ~146x |
| Total synthesis time (est.) | 443s | <10s | >44x |

---

## Implementation Plan

1. `storage.rs`: Add `get_all_hebbian_links_bulk()` + `get_all_memory_entities_bulk()` (+30 lines)
2. `clustering.rs`: VP-tree implementation (+80 lines)
3. `clustering.rs`: Two-stage k-NN in `cluster()` method (+40 lines)
4. `synthesis/cluster.rs`: Switch to bulk loading (-20/+5 lines)
5. Tests: VP-tree + bulk method + consistency tests (+80 lines)
6. Run full test suite, verify all 708 existing tests pass

**Total delta:** ~+215 lines, no new dependencies.

---

## Notes

- **VP-tree 距离函数必须是 metric（满足三角不等式）。** Cosine distance (1 - cos_sim) 不满足三角不等式，不能用。改用 Euclidean distance on L2-normalized vectors：`||a-b|| = sqrt(2(1 - cos(a,b)))`，满足 metric 条件且与 cosine distance 单调等价。运行时 assert L2 norm ≈ 1.0。
- k' (over-fetch factor) 是个可调参数。默认 k'=3k 应该足够。太大浪费计算，太小丢关键边。
- Entity co-occurring pairs 注入是对 ANN+Hebbian 候选集的补充。如果实际 profiling 显示 entity pairs 占比极小（<1%），可以省略但需记录为 known gap。
- 这个改动不影响 Infomap 本身的质量 —— 只是改变了喂给 Infomap 的 edge list 的构建方式。如果 ANN 足够好（k' 足够大 + Hebbian/Entity 注入完整），edge list 应该基本一致。
- **NaN 安全**：VP-tree 内部的距离比较用 `partial_cmp(...).unwrap_or(Ordering::Greater)`，无需 `ordered-float` crate。
- **两条调用路径都受益**：`synthesis/cluster.rs` (MultiSignal) 和 `compiler/discovery.rs` (EmbeddingOnly) 都经过 `InfomapClusterer::cluster()`，VP-tree 改动一次覆盖两者。
