# Episodic → Semantic Distillation

> 把重复的情景记忆自动提炼为持久的语义记忆

## 问题

Agent 跟用户聊了 30 次 Docker 部署相关的话题，产生了 30 条 episodic memory：

```
[episodic] "2/1: user said they prefer Docker for deployment"
[episodic] "2/5: discussed Docker compose for staging"
[episodic] "2/8: user asked about Docker volume mounts"
[episodic] "2/12: deployed new service with Docker"
[episodic] "2/15: user mentioned switching from Docker to Podman"
...
```

问题：
1. **检索噪音** — query "deployment" 返回 30 条相似但独立的记忆
2. **无提炼** — agent 不知道 "这个用户是个重度 Docker 用户"
3. **存储膨胀** — 重复信息占用空间和搜索时间

人脑的做法：海马体的 episodic memory 经过反复 replay，提炼成 neocortex 的 semantic memory。你不记得每次系鞋带的具体经历，但你"知道"怎么系鞋带。

## 认知科学基础

| 理论 | 来源 | 应用 |
|------|------|------|
| **Complementary Learning Systems** | McClelland et al. (1995) | 海马体(快速学习 episodic) + 新皮层(缓慢提炼 semantic) |
| **Memory Consolidation** | Squire & Alvarez (1995) | 反复 replay → episodic 脱离 context → 变成 semantic |
| **Schema Theory** | Bartlett (1932) | 重复经验形成抽象 schema，新经验 assimilate 或 accommodate |
| **Gist Extraction** | Brainerd & Reyna (2002) | 人类同时存储 verbatim + gist，gist 更持久 |

## 设计

### 触发条件

在 `consolidate()` 过程中增加 distillation 步骤。触发条件：

```python
def should_distill(cluster: list[MemoryEntry]) -> bool:
    """判断一组 episodic 记忆是否该被提炼"""
    return (
        len(cluster) >= DISTILL_MIN_EPISODES      # 至少 5 条相关 episodic
        and cluster_age_span(cluster) >= 7 * 86400  # 跨度至少 7 天
        and avg_access_count(cluster) >= 3           # 平均被访问过 3+ 次
    )
```

### 流程

```
consolidate() 被调用
    │
    ▼
┌─────────────────────────────────────────────────┐
│ 1. CLUSTER DETECTION                             │
│    找到语义相似的 episodic memories 簇             │
│    方法：Hebbian links + FTS overlap              │
│    不需要 embedding — 用已有的 Hebbian 图          │
└────────────────────┬────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────┐
│ 2. TRIGGER CHECK                                 │
│    should_distill(cluster) → True/False          │
└────────────────────┬────────────────────────────┘
                     │ True
                     ▼
┌─────────────────────────────────────────────────┐
│ 3. GIST EXTRACTION                               │
│    从 cluster 中提取语义摘要                       │
│    ├─ Option A: LLM summarization (if available)  │
│    └─ Option B: TF-IDF + frequency (offline)      │
└────────────────────┬────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────┐
│ 4. SEMANTIC MEMORY CREATION                      │
│    创建新的 semantic 类型 memory                   │
│    ├─ type = SEMANTIC                             │
│    ├─ layer = L2_CORE                             │
│    ├─ importance = max(cluster.importance)         │
│    ├─ source_episodes = [episode_ids]             │
│    └─ core_strength = high (pre-consolidated)     │
└────────────────────┬────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────┐
│ 5. EPISODE DEMOTION                              │
│    原始 episodic memories:                        │
│    ├─ 降级到 L4_ARCHIVE (不删除)                   │
│    ├─ 降低 working_strength                       │
│    └─ 保留最新 1-2 条作为 "vivid episodes"         │
└─────────────────────────────────────────────────┘
```

### Pattern Classification（偏好 vs 近期热点 vs 条件性）

提炼不应盲目假设 "偏好"。30 次提到 Docker 可能是真偏好、也可能只是最近项目在用。

```python
class PatternType(Enum):
    PREFERENCE = "preference"           # 长期 + 分散 + 多 context
    CURRENT_FOCUS = "current_focus"     # 集中在最近 2 周
    CONTEXTUAL = "contextual"          # 只在特定场景出现
    OBSERVATION = "observation"         # 模式不确定

def classify_pattern(cluster: list[MemoryEntry]) -> PatternType:
    """区分偏好 vs 近期热点 vs 条件性使用"""
    
    time_span = max(e.created_at for e in cluster) - min(e.created_at for e in cluster)
    recent_ratio = sum(1 for e in cluster 
                       if (now() - e.created_at).days <= 14) / len(cluster)
    
    # context 多样性：通过 Hebbian 邻居的多样性估算
    # 如果这些 episodic 记忆连接到很多不同 topic 的记忆 = 多 context
    context_diversity = count_unique_hebbian_neighborhoods(cluster)
    
    if time_span > timedelta(days=30) and recent_ratio < 0.6 and context_diversity >= 3:
        return PatternType.PREFERENCE
    elif recent_ratio > 0.7:
        return PatternType.CURRENT_FOCUS
    elif context_diversity <= 1:
        return PatternType.CONTEXTUAL
    else:
        return PatternType.OBSERVATION
```

**不同 pattern 类型的语义记忆有不同衰减率**：

```python
DECAY_BY_PATTERN = {
    PatternType.PREFERENCE:    0.3,   # 很慢 — 真偏好不会轻易变
    PatternType.CURRENT_FOCUS: 0.7,   # 较快 — 热点会过去
    PatternType.CONTEXTUAL:    0.5,   # 中等
    PatternType.OBSERVATION:   0.6,   # 偏快 — 不确定的模式
}
```

**关键 insight**：`CURRENT_FOCUS` 类型的语义记忆如果后续 2 周没有新的相关 episodic 加入，应该自动降级。这模拟了人类的 "最近在忙什么" 记忆 — 项目结束后自然淡化。

### Cluster Detection（不依赖 embedding）

```python
def find_distillable_clusters(store: Store) -> list[list[MemoryEntry]]:
    """用 Hebbian 图 + FTS 找到可提炼的 episodic 簇"""
    
    episodic = store.get_by_type(MemoryType.EPISODIC)
    
    # 建邻接图：Hebbian link strength > threshold
    graph = {}
    for mem in episodic:
        neighbors = store.get_hebbian_neighbors(mem.id, min_strength=0.3)
        graph[mem.id] = [n.id for n in neighbors if n.type == MemoryType.EPISODIC]
    
    # Connected components = clusters
    clusters = connected_components(graph)
    
    # 过滤：只保留满足触发条件的
    return [c for c in clusters if should_distill(c)]
```

**关键洞察**：Hebbian links 已经告诉我们哪些记忆经常被一起 recall — 这正好就是 "同一个主题" 的信号。不需要额外的 embedding 或 clustering 算法。

### Gist Extraction

#### Option A: LLM-assisted（推荐，如果 LLM 可用）

```python
def extract_gist_llm(cluster: list[MemoryEntry], llm) -> str:
    """用 LLM 从一组 episodic 记忆中提取语义摘要"""
    
    episodes = "\n".join([
        f"[{e.created_at.date()}] {e.content}" 
        for e in sorted(cluster, key=lambda x: x.created_at)
    ])
    
    prompt = f"""Below are {len(cluster)} related memory episodes from different times.
Extract ONE concise semantic fact or preference that captures the common theme.
Do NOT include dates or specific events — extract the lasting knowledge.

Episodes:
{episodes}

Semantic summary (one sentence):"""
    
    return llm.complete(prompt).strip()
```

**示例输入**：
```
[2/1] user said they prefer Docker for deployment
[2/5] discussed Docker compose for staging
[2/8] user asked about Docker volume mounts
[2/12] deployed new service with Docker
[2/15] user mentioned considering Podman but stayed with Docker
```

**示例输出（根据 pattern 类型不同）**：
```
# PREFERENCE (跨 30+ 天，多 context):
"User consistently prefers Docker for deployment workflows (6 months, across 4 projects)"

# CURRENT_FOCUS (集中在最近 2 周):
"User is currently working heavily with Docker for a deployment project"

# CONTEXTUAL (只在特定场景):
"User uses Docker specifically for production deployment; dev environment uses other tools"

# OBSERVATION (模式不确定):
"User has discussed Docker in 30 conversations over 3 weeks"
```

#### Option B: Offline extraction（无 LLM 依赖）

```python
def extract_gist_offline(cluster: list[MemoryEntry]) -> str:
    """纯本地 gist 提取 — TF-IDF + 最高 activation 条目"""
    
    # 1. 提取高频关键词（TF-IDF across cluster vs all memories）
    keywords = tfidf_keywords(cluster, top_k=5)
    
    # 2. 选 activation 最高的一条作为 base
    base = max(cluster, key=lambda e: e.core_strength + e.working_strength)
    
    # 3. 拼接
    return f"[distilled from {len(cluster)} episodes] {base.content} (keywords: {', '.join(keywords)})"
```

### 数据模型变更

```python
class MemoryEntry:
    # 现有字段...
    
    # 新增
    source_episodes: list[str] | None = None   # 来源 episodic IDs
    distilled_from_count: int = 0               # 提炼自多少条
    last_distillation: datetime | None = None   # 最后提炼时间
```

```sql
-- 新增字段
ALTER TABLE memories ADD COLUMN source_episodes TEXT;  -- JSON array of IDs
ALTER TABLE memories ADD COLUMN distilled_from_count INTEGER DEFAULT 0;
ALTER TABLE memories ADD COLUMN last_distillation TIMESTAMP;
```

### API

```python
class Memory:
    def consolidate(self, distill: bool = True, llm=None):
        """扩展现有 consolidate，加入 distillation"""
        
        # 原有逻辑：layer promotion, synaptic downscaling, Hebbian decay
        super().consolidate()
        
        if distill:
            clusters = find_distillable_clusters(self._store)
            for cluster in clusters:
                if llm:
                    gist = extract_gist_llm(cluster, llm)
                else:
                    gist = extract_gist_offline(cluster)
                
                # 创建 semantic memory
                semantic_id = self.add(
                    gist, 
                    type=MemoryType.SEMANTIC,
                    importance=max(e.importance for e in cluster),
                    metadata={"source_episodes": [e.id for e in cluster]}
                )
                
                # 降级原始 episodes（保留最新 2 条）
                sorted_eps = sorted(cluster, key=lambda x: x.created_at, reverse=True)
                for ep in sorted_eps[2:]:  # 保留最新 2 条
                    self._store.update(ep.id, layer=MemoryLayer.L4_ARCHIVE)
                    self._store.update(ep.id, working_strength=0.1)
    
    def distillation_stats(self) -> dict:
        """返回 distillation 统计"""
        return {
            "semantic_memories": self._store.count_by_type(MemoryType.SEMANTIC),
            "distilled_total": sum(e.distilled_from_count for e in self._store.get_by_type(MemoryType.SEMANTIC)),
            "archived_episodes": self._store.count_by_layer(MemoryLayer.L4_ARCHIVE),
            "pending_clusters": len(find_distillable_clusters(self._store)),
        }
```

## 参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `DISTILL_MIN_EPISODES` | 5 | 最少多少条 episodic 才触发提炼 |
| `DISTILL_MIN_AGE_DAYS` | 7 | 簇的时间跨度至少多少天 |
| `DISTILL_MIN_AVG_ACCESS` | 3 | 簇内记忆平均被访问次数 |
| `DISTILL_KEEP_VIVID` | 2 | 保留最近几条 episodic 不归档 |
| `DISTILL_HEBBIAN_THRESHOLD` | 0.3 | Hebbian link 强度阈值（用于 clustering） |

## Preset 适配

| Preset | min_episodes | min_age | 说明 |
|--------|-------------|---------|------|
| `chatbot` | 3 | 3 days | 快速提炼，减少噪音 |
| `personal-assistant` | 5 | 7 days | 标准节奏 |
| `researcher` | 10 | 14 days | 保守提炼，保留更多 episode |
| `task-agent` | N/A | N/A | 禁用（任务太短，不需要） |

## 测试计划

```python
def test_distillation_basic():
    """5+ 条相关 episodic → 1 条 semantic"""
    mem = Memory(":memory:")
    # 添加 5 条 Docker 相关记忆，间隔 7+ 天
    # 模拟 recall 使它们 co-activate → 形成 Hebbian links
    # 调用 consolidate(distill=True)
    # 验证：1 条新 SEMANTIC memory 出现
    # 验证：旧 episodes 降级到 ARCHIVE（最新 2 条除外）

def test_distillation_threshold():
    """不满足条件的 cluster 不被提炼"""
    # 只有 3 条 → 不触发
    # 时间跨度 < 7 天 → 不触发
    # 平均 access < 3 → 不触发

def test_distillation_preserves_vivid():
    """最新 N 条 episode 保留在 WORKING/CORE"""
    # 验证 DISTILL_KEEP_VIVID 条最新的不被归档

def test_semantic_retrieval_priority():
    """semantic memory 在检索中优先于归档的 episodes"""
    # 提炼后查询 → semantic 结果排在 archived episodes 前面

def test_distillation_offline():
    """无 LLM 时 gist extraction 仍然工作"""
    # 用 Option B (TF-IDF) 验证
```

---

*Document created: 2026-03-09*
*Cognitive basis: Complementary Learning Systems (McClelland 1995), Schema Theory (Bartlett 1932)*
