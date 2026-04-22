# ISS-015 [improvement] [P1] [closed]

**标题**: 聚类算法升级 — Union-Find 连通分量 → Infomap 社区检测
**发现日期**: 2026-04-18
**关闭日期**: 2026-04-18
**发现者**: potato + RustClaw
**组件**: synthesis/cluster.rs, clustering.rs, compiler/discovery.rs
**跨项目引用**: gid-core (infomap-rs 实现已存在)

---

## 问题

engram 目前有 **两套独立的聚类实现**，都有根本性缺陷：

### 1. `synthesis/cluster.rs`（engram core）
- **4 信号**：Hebbian 权重 + 实体 Jaccard + embedding cosine + 时间接近度
- 加权组合 → 阈值过滤 → **Union-Find 连通分量**
- 信号丰富，但聚类算法是最简单的连通分量

### 2. `compiler/discovery.rs`（KC）
- **1 信号**：embedding cosine
- **单链接凝聚聚类**（agglomerative single-linkage）
- hardcoded `similarity_threshold = 0.5`
- 完全忽略 Hebbian、实体、时间信号
- **不应该自己写聚类，应该用 engram core 的**

### Union-Find 的具体问题

**问题 1：链式污染（single-linkage 经典缺陷）**

```
记忆 A: "Rust 异步编程"
记忆 B: "Rust 错误处理"
记忆 C: "Go 错误处理"
记忆 D: "Go 并发模型"

A-B 相似（Rust）, B-C 相似（错误处理）, C-D 相似（Go）
→ Union-Find 把 A-B-C-D 全连成一个簇
→ topic = "编程语言"，太泛，没用

Infomap 会识别出 {A,B} 和 {C,D} 两个社区
```

**问题 2：阈值敏感**
- 调高 → 碎成小簇（topic 太碎）
- 调低 → 合成巨型簇（topic 太泛）
- 不同密度的记忆区域需要不同阈值，但只有一个全局阈值
- Infomap 无阈值参数，信息论最优自适应

**问题 3：无层次结构**
- Union-Find 输出扁平（在/不在）
- 知识天然有层次："机器学习" > "强化学习" > "PPO"
- Infomap 层次模式可输出多级 topic 嵌套

---

## 方案

### 已有资源
- `infomap-rs` 在 gid-core 中已有完整实现（`infer/clustering.rs`，1945 行，17 tests）
- 通用加权网络，不绑定代码——代码特定的只是边权策略
- 换成知识图谱只需换边权

### 架构改动

**Step 1：KC 统一使用 engram core 聚类（不改算法）**
```
compiler/discovery.rs:
  - 删掉自己的聚类逻辑（agglomerative single-linkage）
  - 调用 synthesis::cluster::discover_clusters()
  - 只负责 MemoryCluster → TopicCandidate 映射
```
- 零风险，立即提升质量（1 信号 → 4 信号）
- KC 不写聚类，KC 消费聚类结果

**Step 2：将 synthesis/cluster.rs 内核从 Union-Find 换成 Infomap**
```
现在：4 信号 → composite score → 阈值过滤 → Union-Find
换后：4 信号 → composite score → 直接当边权 → Infomap

认知信号全部保留，只改聚类内核
去掉 cluster_threshold 这个 hard cut
Infomap 接收全图（正权边），自己决定最优划分
```

**依赖选项**：
- 方案 A：engram 直接依赖 `infomap-rs` crate（从 gid-core 抽出独立 crate）
- 方案 B：engram 依赖 gid-core（引入不必要依赖，不推荐）
- 方案 C：将 Infomap 核心算法复制到 engram（代码重复，不推荐）
- **推荐方案 A**：infomap-rs 本身就是通用算法，应独立发布

### 边权映射

```rust
// 现有 4 信号组合（保留不变）
fn composite_score(a: &Memory, b: &Memory) -> f64 {
    let hebbian = hebbian_weight(a, b);        // co-recall 频率
    let entity  = entity_jaccard(a, b);        // 实体重叠度
    let embed   = embedding_cosine(a, b);      // 语义相似度
    let time    = time_proximity(a, b);         // 时间接近度
    
    // 加权组合 → 直接作为 Infomap 边权
    w_h * hebbian + w_e * entity + w_c * embed + w_t * time
}
```

### 输出变化

- 输入不变：`Vec<MemoryRecord>` + embeddings + Hebbian links
- 输出结构不变：`Vec<MemoryCluster>`
- 新增：层次信息（cluster 可包含 sub-clusters）
- 下游（KC、synthesis）无需改动

---

## 优先级与依赖

- **Step 1（KC 统一聚类）**: 无依赖，可立即做
- **Step 2（Infomap 替换）**: 依赖 infomap-rs 独立发布
- 与 ISS-009（Entity 索引）配合：entity overlap 信号质量提升 → 聚类质量提升

## 注意事项

- 记忆量 < 50 条时 Infomap 与 Union-Find 差别不大，需有 fallback
- Infomap 计算量更大，但 gid 实现已优化
- Hebbian 信号依赖用户实际 recall 使用量——纯"扔素材"模式下 Hebbian 数据稀疏，其他三信号为主


