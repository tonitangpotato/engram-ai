# Paper Review: NeuromemoryAI

## 📋 待修复的问题

### 1. TODO 未填充
- **Abstract**: `[TODO]\%` - 需要填入具体的实验数据
- **Section 5.4 Results**: `[TODO: Run experiments and report results]` - 需要实验结果

### 2. 引用格式
- HippoRAG引用 `\citep{yu2024hipporag}` 有 `others` 需要完整作者列表
- 部分引用年份可能需要核实（如 piantadosi2016 但 bib 里是 2014）

### 3. 小问题
- Figure 1 用 verbatim 画架构图不够专业，建议用 TikZ 或真正的图片
- "~500 lines of code" - 需要验证实际行数

## ➕ 建议增加的内容

### 1. 实验结果 (Section 5.4)
需要跑 benchmark 对比：
- **Multi-session recall accuracy**: 跨 session 回忆用户偏好
- **Relevance vs recency tradeoff**: 旧但相关 vs 新但不相关
- **Forgetting effectiveness**: 有 forgetting vs 无 forgetting 的 signal-to-noise
- **Hebbian emergence**: 自动形成的链接数量和质量

### 2. 性能数据
- 每次 recall 的延迟 (ms)
- 内存增长曲线 (有 forgetting vs 无)
- Consolidation 时间

### 3. 真实案例
加一个具体的使用场景，比如：
- Chatbot 记住用户 5 天前说的偏好
- 展示 Hebbian links 自动形成的可视化

## 📊 Benchmark 设计

### Benchmark 1: Multi-Session Continuity
```
Setup:
- 10 sessions over 7 simulated days
- Session 1: Add user preferences (name, job, likes)
- Sessions 2-9: Random conversations
- Session 10: Query for early preferences

Metrics:
- Recall@1: Is the correct preference the top result?
- Recall@3: Is it in top 3?
- MRR: Mean Reciprocal Rank
```

### Benchmark 2: Relevance vs Recency
```
Setup:
- Add highly relevant memory 30 days ago
- Add tangentially related memory 1 hour ago
- Query with context matching old memory

Metrics:
- Which ranks higher?
- Activation score comparison
```

### Benchmark 3: Forgetting Benefits
```
Setup:
- Add 100 memories (20 relevant, 80 noise)
- Run with forgetting OFF vs ON
- Query for the 20 relevant topics

Metrics:
- Precision@5: What % of top-5 are relevant?
- Storage size after 30 days
```

### Benchmark 4: Hebbian Emergence
```
Setup:
- Add memories about 3 topics (ML, cooking, travel)
- Simulate usage that co-retrieves within-topic memories
- No manual linking

Metrics:
- # of Hebbian links formed
- % of links that are within-topic (correct)
- # of cross-topic links (noise)
```

## 🏃 运行方法

See `benchmarks/run_benchmark.py` for executable benchmark.

```bash
cd /Users/potato/clawd/projects/agent-memory-prototype
python benchmarks/run_benchmark.py --all
python benchmarks/run_benchmark.py --task multi-session
python benchmarks/run_benchmark.py --compare mem0  # 需要 mem0 API key
```
