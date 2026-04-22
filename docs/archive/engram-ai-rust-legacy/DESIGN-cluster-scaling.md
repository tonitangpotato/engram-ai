# DESIGN: Cluster Discovery Scaling (VP-tree + Incremental)

> 让 discover_clusters 从 "百条级" 扩展到 "十万条级"，支持多 agent 场景。

## 现状分析

### 当前 pipeline (cluster.rs, ~980 行)

```
Step 1: 取全量候选记忆 (storage.all() → filter)
Step 2: 预计算信号 maps (Hebbian、Entity、Embedding 全量加载)
Step 3: 生成 candidate pairs (仅 Hebbian link + shared entity 的 pairs)
Step 4: 计算 composite score，threshold 过滤
Step 5: sparsify → Infomap → communities
Step 6: build MemoryCluster structs
```

### 三个问题

**P1: Embedding 信号被浪费。** Step 3 只从 Hebbian links 和 shared entities 生成 candidate pairs。两条记忆如果 embedding 很相似但既没有 Hebbian link 也没有共享 entity → 永远不会被配对。Embedding weight 在 composite score 里占 0.2，但对 pair discovery 贡献为零。

**P2: 全量重算。** 每次 consolidate 都跑 `storage.all()` 全量加载 + 全量配对。1000 条记忆时 OK，10000 条时 Hebbian map 构建本身就是 O(n) 次 SQLite 查询，每次返回 O(k) 条 → 总 O(n·k)。加上 entity map、embedding map 全量内存加载。

**P3: VP-tree 之前设计过但没实现。** Engram 记忆里有完整的 VP-tree + Hot/Warm/Cold 三层设计，但代码中没有。

---

## 设计

### §1 VP-tree for ANN k-NN (Step 1)

在 cluster.rs 中实现 VP-tree (Vantage-Point tree)，用于在 embedding 空间快速查找 k 个最近邻。

#### 为什么 VP-tree 不是 HNSW

- VP-tree ~80 行 Rust，零依赖，纯内存结构
- 对 L2 normalized embedding vectors（cosine distance = L2 distance² / 2），VP-tree 表现良好
- 10k-100k 规模足够，百万级再换 HNSW
- 已经 design 过一次，这次直接实现

#### 数据结构

```rust
// cluster.rs 内部，不 pub — 这是实现细节

/// Vantage-Point Tree for approximate nearest-neighbor search
/// on L2-normalized embedding vectors.
///
/// Distance metric: L2 distance (equivalent to cosine distance
/// for normalized vectors: cosine_dist = L2² / 2).
struct VpTree {
    nodes: Vec<VpNode>,
}

struct VpNode {
    /// Index into the original points array
    point_idx: usize,
    /// Median distance to vantage point (split threshold)
    threshold: f32,
    /// Left child: points with distance <= threshold
    left: Option<usize>,  // index into nodes vec
    /// Right child: points with distance > threshold
    right: Option<usize>,
}
```

#### 构建: O(n log n)

```
build(points: &[(usize, &[f32])]) -> VpTree
  - 随机选 vantage point（第一个元素，不需要 optimal selection）
  - 算所有点到 vantage 的 L2 distance
  - median 作为 threshold
  - 左子树: distance <= median
  - 右子树: distance > median
  - 递归
```

#### 查询: O(log n) amortized

```
query_k_nearest(tree, query_point, k) -> Vec<(usize, f32)>
  - 维护 max-heap (size k) 存当前 k 个最近邻
  - 算 query 到 vantage 的 distance d
  - if d <= threshold: 先搜左子树，如果 heap 未满或 d + heap.max > threshold 则也搜右子树
  - if d > threshold: 先搜右子树，如果 heap 未满或 threshold + heap.max > d 则也搜左子树
  - 剪枝条件保证平均只访问 O(log n) 个节点
```

#### 集成到 discover_clusters

**Step 3 增加第三个 pair source:**

```rust
// 现有: Hebbian pairs + Entity pairs
// 新增: Embedding ANN pairs

// 用全量 embedding_map 构建 VP-tree
let points: Vec<(usize, &[f32])> = embedding_map.iter()
    .enumerate()
    .map(|(i, (_, emb))| (i, emb.as_slice()))
    .collect();
let vp_tree = VpTree::build(&points);

// 对每个候选记忆，找 k 个 embedding 最近邻加入 candidate_pairs
let ann_k = config.max_neighbors_per_node.unwrap_or_else(|| {
    let adaptive = (n as f64).sqrt().round() as usize;
    adaptive.clamp(5, 30)
});
for (i, (id, _)) in embedding_map.iter().enumerate() {
    let neighbors = vp_tree.query_k_nearest(i, ann_k);
    for (j, _dist) in neighbors {
        let neighbor_id = &embedding_ids[j];
        let pair = ordered_pair(id, neighbor_id);
        candidate_pairs.insert(pair);
    }
}
```

**效果:** 之前 n=1000 时 candidate pairs ≈ Hebbian + Entity pairs（可能几百到几千对）。加 ANN 后，每个节点额外贡献 k 个 pairs，总共 ~n·k pairs（15k），但这些是真正 embedding 近的 pairs，不是 O(n²) 暴力。

### §2 Incremental Clustering (Step 2)

#### 核心思路: Hot / Warm / Cold 三层

Infomap 本身不支持增量更新（每次运行都是全局优化）。但我们可以把"全量 Infomap"变成低频操作：

```
Hot  (每条新记忆):  O(log n) — ANN 找最近 cluster，直接归入
Warm (每 N 条新增): O(m log m) — 对 dirty clusters 局部 re-cluster
Cold (周期性):      O(n log n) — 全量 Infomap 校准
```

#### 持久化: ClusterState

在 SQLite 中新增一张表存聚类状态：

```sql
CREATE TABLE IF NOT EXISTS cluster_state (
    -- 上次全量聚类的 metadata
    last_full_cluster_at   TEXT,        -- ISO 8601 timestamp
    last_full_memory_count INTEGER,     -- 全量时记忆总数
    -- 增量跟踪
    pending_memory_ids     TEXT,        -- JSON array of memory IDs added since last full
    dirty_cluster_ids      TEXT,        -- JSON array of cluster IDs that got hot-assigned
    -- cluster assignments
    -- (separate table below)
    version                INTEGER DEFAULT 1
);

CREATE TABLE IF NOT EXISTS cluster_assignments (
    memory_id   TEXT PRIMARY KEY,
    cluster_id  TEXT NOT NULL,
    assigned_at TEXT NOT NULL,           -- ISO 8601
    method      TEXT NOT NULL,           -- 'full', 'hot', 'warm'
    confidence  REAL NOT NULL DEFAULT 1.0 -- hot assignment confidence (cosine to centroid)
);

CREATE TABLE IF NOT EXISTS cluster_centroids (
    cluster_id  TEXT PRIMARY KEY,
    centroid    BLOB NOT NULL,           -- f32 embedding vector
    member_count INTEGER NOT NULL,
    updated_at  TEXT NOT NULL
);
```

#### Hot Path: assign_to_nearest_cluster()

```rust
pub fn assign_new_memory(
    storage: &Storage,
    memory_id: &str,
    embedding: &[f32],
    config: &ClusterDiscoveryConfig,
) -> Result<HotAssignResult, Error> {
    // 1. 从 cluster_centroids 加载所有 centroid vectors
    let centroids = storage.get_cluster_centroids()?;
    
    if centroids.is_empty() {
        // 没有现有聚类，加入 pending
        return Ok(HotAssignResult::Pending);
    }
    
    // 2. 找最近 centroid (brute force on centroids, not memories — 通常 < 100 个 cluster)
    let (best_cluster, best_sim) = centroids.iter()
        .map(|(cid, centroid)| (cid, cosine_similarity(embedding, centroid)))
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .unwrap();
    
    // 3. 阈值判断
    if best_sim >= config.hot_assign_threshold {  // e.g., 0.6
        // 归入现有 cluster，标记 cluster 为 dirty
        storage.assign_to_cluster(memory_id, best_cluster, "hot", best_sim)?;
        // 增量更新 centroid: new_centroid = (old * n + new) / (n+1)
        storage.update_centroid_incremental(best_cluster, embedding)?;
        storage.mark_cluster_dirty(best_cluster)?;
        Ok(HotAssignResult::Assigned { 
            cluster_id: best_cluster.clone(), 
            confidence: best_sim 
        })
    } else {
        // 不够近，加入 pending 等 warm/cold path
        storage.add_pending_memory(memory_id)?;
        Ok(HotAssignResult::Pending)
    }
}
```

**复杂度:** O(C) where C = cluster 数量（通常 < 100）。每条新记忆几乎零成本。

#### Warm Path: recluster_dirty()

```rust
pub fn recluster_dirty(
    storage: &Storage,
    config: &ClusterDiscoveryConfig,
    embedding_model: Option<&str>,
) -> Result<WarmReclusterReport, Error> {
    // 1. 取 dirty cluster IDs + pending memory IDs
    let dirty_ids = storage.get_dirty_cluster_ids()?;
    let pending_ids = storage.get_pending_memory_ids()?;
    
    if dirty_ids.is_empty() && pending_ids.is_empty() {
        return Ok(WarmReclusterReport::NothingToDo);
    }
    
    // 2. 收集所有相关记忆: dirty clusters 的成员 + pending
    let mut involved_ids: HashSet<String> = HashSet::new();
    for cid in &dirty_ids {
        let members = storage.get_cluster_members(cid)?;
        involved_ids.extend(members);
    }
    involved_ids.extend(pending_ids);
    
    // 3. 只对这些记忆跑局部 discover_clusters
    let local_clusters = discover_clusters_subset(
        storage, 
        &involved_ids, 
        config, 
        embedding_model
    )?;
    
    // 4. 更新 cluster_assignments + centroids
    //    - 删除旧 dirty cluster 的 assignments
    //    - 写入新 cluster assignments
    //    - 更新 centroids
    //    - 清除 dirty + pending 标记
    storage.replace_clusters(&dirty_ids, &local_clusters)?;
    
    Ok(WarmReclusterReport {
        clusters_reclustered: dirty_ids.len(),
        pending_assigned: involved_ids.len(),
        new_clusters: local_clusters.len(),
    })
}
```

**复杂度:** O(m log m) where m = dirty members + pending，远小于 n。

**触发时机:**
- 每 100 条新记忆（可配置）
- consolidate() 调用时
- 手动触发

#### Cold Path: full recluster

就是现有的 `discover_clusters()`，不变。

**触发时机:**
- pending 累积超过总量 20%
- 每 1000 条新增记忆
- 手动触发
- hot assignment confidence 持续偏低（说明 cluster 结构漂移了）

#### discover_clusters_subset()

新增函数，和 `discover_clusters()` 几乎一样，但接受一个 memory ID 集合而非全量：

```rust
pub fn discover_clusters_subset(
    storage: &Storage,
    memory_ids: &HashSet<String>,
    config: &ClusterDiscoveryConfig,
    embedding_model: Option<&str>,
) -> Result<Vec<MemoryCluster>, Error> {
    // 和 discover_clusters 相同的 Step 1-6，
    // 但 Step 1 不调 storage.all()，而是只加载 memory_ids 指定的记忆
    // Step 2-6 完全不变
}
```

为避免代码重复，把 discover_clusters 的核心逻辑提取成内部函数，两个入口共享：

```rust
// 公开 API (不变)
pub fn discover_clusters(...) -> Result<Vec<MemoryCluster>, Error> {
    let all = storage.all()?;
    let candidates = all.iter().filter(|m| ...).collect();
    discover_clusters_inner(storage, &candidates, config, embedding_model)
}

// 新增 API
pub fn discover_clusters_subset(
    storage: &Storage,
    memory_ids: &HashSet<String>,
    config: &ClusterDiscoveryConfig,
    embedding_model: Option<&str>,
) -> Result<Vec<MemoryCluster>, Error> {
    let records = storage.get_memories_by_ids(memory_ids)?;
    let candidates = records.iter().filter(|m| ...).collect();
    discover_clusters_inner(storage, &candidates, config, embedding_model)
}

// 共享内部实现
fn discover_clusters_inner(...) -> Result<Vec<MemoryCluster>, Error> {
    // 现有 Step 2-6，完全不变
}
```

### §3 配置扩展

```rust
pub struct ClusterDiscoveryConfig {
    // ... 现有字段 ...
    
    // === NEW: Incremental clustering ===
    
    /// Hot assign threshold: cosine similarity to nearest centroid.
    /// Above this → assign to cluster. Below → pending.
    /// `None` = default 0.6.
    #[serde(default)]
    pub hot_assign_threshold: Option<f64>,
    
    /// Warm recluster trigger: recluster dirty clusters every N new memories.
    /// `None` = default 100.
    #[serde(default)]
    pub warm_recluster_interval: Option<usize>,
    
    /// Cold recluster trigger: full recluster when pending exceeds this
    /// fraction of total memories.
    /// `None` = default 0.2 (20%).
    #[serde(default)]
    pub cold_recluster_ratio: Option<f64>,
    
    // === NEW: VP-tree ANN ===
    
    /// Whether to use VP-tree ANN for embedding-based pair discovery.
    /// `None` = default true (auto-enable when embedding_model is set).
    #[serde(default)]
    pub use_ann: Option<bool>,
}
```

### §4 Storage API 扩展

```rust
impl Storage {
    // cluster_state table
    pub fn init_cluster_tables(&self) -> Result<(), Error>;
    pub fn get_cluster_centroids(&self) -> Result<Vec<(String, Vec<f32>)>, Error>;
    pub fn assign_to_cluster(&self, memory_id: &str, cluster_id: &str, method: &str, confidence: f64) -> Result<(), Error>;
    pub fn update_centroid_incremental(&self, cluster_id: &str, new_embedding: &[f32]) -> Result<(), Error>;
    pub fn mark_cluster_dirty(&self, cluster_id: &str) -> Result<(), Error>;
    pub fn get_dirty_cluster_ids(&self) -> Result<Vec<String>, Error>;
    pub fn add_pending_memory(&self, memory_id: &str) -> Result<(), Error>;
    pub fn get_pending_memory_ids(&self) -> Result<Vec<String>, Error>;
    pub fn get_cluster_members(&self, cluster_id: &str) -> Result<Vec<String>, Error>;
    pub fn replace_clusters(&self, old_ids: &[String], new_clusters: &[MemoryCluster]) -> Result<(), Error>;
    pub fn get_memories_by_ids(&self, ids: &HashSet<String>) -> Result<Vec<MemoryRecord>, Error>;
    pub fn clear_pending_and_dirty(&self) -> Result<(), Error>;
}
```

### §5 集成点

#### consolidate() 改造

```rust
// engine.rs
fn consolidate(&self, storage: &mut Storage, settings: &SynthesisSettings) -> Result<Report> {
    // Phase 1: Warm recluster (if needed)
    let pending_count = storage.get_pending_memory_ids()?.len();
    let total_count = storage.count_memories()?;
    
    let should_cold = match config.cold_recluster_ratio {
        Some(ratio) => pending_count as f64 / total_count as f64 > ratio,
        None => pending_count as f64 / total_count as f64 > 0.2,
    };
    
    let clusters = if should_cold {
        // Cold: full recluster
        log::info!("cold recluster: {} pending / {} total", pending_count, total_count);
        let clusters = discover_clusters(storage, &config)?;
        storage.save_full_cluster_state(&clusters)?;
        clusters
    } else if pending_count > 0 {
        // Warm: recluster dirty + pending only
        log::info!("warm recluster: {} pending, {} dirty clusters", pending_count, dirty_count);
        recluster_dirty(storage, &config, embedding_model)?;
        // Return all clusters (including untouched ones)
        storage.get_all_clusters()?
    } else {
        // Nothing new, use cached clusters
        storage.get_all_clusters()?
    };
    
    // Phase 2: Gate check + synthesis (unchanged)
    for cluster in &clusters { ... }
}
```

#### add() hook for hot path

```rust
// 在 Storage::add() 或 engine 层:
// 每次 add 新记忆后，如果有 embedding，走 hot path
pub fn on_memory_added(
    storage: &Storage,
    memory_id: &str,
    embedding: Option<&[f32]>,
    config: &ClusterDiscoveryConfig,
) -> Result<(), Error> {
    if let Some(emb) = embedding {
        let result = assign_new_memory(storage, memory_id, emb, config)?;
        log::debug!("hot assign: {:?}", result);
    } else {
        storage.add_pending_memory(memory_id)?;
    }
    Ok(())
}
```

---

## 复杂度对比

| 操作 | 现在 | 改后 |
|------|------|------|
| 新增 1 条记忆 | 无（等 consolidate） | O(C) hot assign |
| consolidate (1000 条) | O(n·k) 全量 | O(m log m) warm, m << n |
| consolidate (10000 条) | O(n·k) 全量 ~10s+ | O(m log m) warm ~0.1s |
| pair discovery | Hebbian + Entity only | + ANN O(n log n) |
| 首次聚类 | O(n·k) | O(n log n) (VP-tree build + query) |

---

## 实现顺序

### Task 1: VP-tree 实现 + ANN pair discovery
1. 在 cluster.rs 内实现 VpTree struct (~80 行)
2. Step 3 增加 ANN pairs source
3. 测试: 验证 embedding-only 的 pairs 能被发现

### Task 2: 内部重构 — discover_clusters_inner 提取
1. 把核心逻辑提到 discover_clusters_inner
2. discover_clusters 和 discover_clusters_subset 共享
3. 全量测试不 regress

### Task 3: Storage 表 + API
1. cluster_state / cluster_assignments / cluster_centroids 三张表
2. init_cluster_tables() 在 Storage::new() 中调用
3. 所有 CRUD methods
4. 单元测试

### Task 4: Hot path
1. assign_new_memory() 实现
2. on_memory_added() hook
3. 测试: 新记忆正确归入最近 cluster

### Task 5: Warm path
1. recluster_dirty() 实现
2. discover_clusters_subset() 连线
3. 测试: dirty clusters 正确 recluster

### Task 6: Cold path + consolidate 集成
1. consolidate() 改造: hot/warm/cold 判断
2. 全量 regression 测试
3. benchmark: 模拟 1000/5000/10000 条记忆场景

---

## 不做什么 (Non-goals)

- **HNSW** — 百万级才需要，VP-tree 在 100k 以下足够
- **多 agent namespace 分区** — Step 3（分区聚类），等真正多 agent 再做
- **跨 agent Hebbian links** — 需要先定义 agent 间共享协议
- **实时 streaming clustering** — hot path 已经是 per-memory O(C)，够了
- **GPU 加速** — 不需要，VP-tree CPU 查询在 100k 级别 < 1ms

---

## 风险

1. **VP-tree 高维退化** — embedding 通常 768/1536 维，VP-tree 在高维下剪枝效率下降。但对 k-NN 查找（不要求精确），approximate 结果可以接受。如果实测发现 VP-tree 查询太慢（>100ms per query at 10k），可以先用随机投影降到 128 维再建树。

2. **Hot assign centroid drift** — 增量更新 centroid 可能漂移（特别是小 cluster）。Warm path 会定期修正，但极端情况下可能导致记忆错误归类。mitigation: confidence 低于阈值的 hot assign 自动标为 pending。

3. **Schema migration** — 新增 3 张 SQLite 表。需要在 Storage::new() 中做 `CREATE TABLE IF NOT EXISTS`，不影响现有数据。
