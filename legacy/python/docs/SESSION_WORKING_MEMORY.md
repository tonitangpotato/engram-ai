# Session Working Memory

## 概述

基于认知科学的 Working Memory 模型，实现会话级别的智能 recall 触发机制。

**核心问题：** Pre-hooks 每条消息都 recall 成本太高，但靠 LLM 自觉遵守 markdown 规则不可靠。

**解决方案：** 不在消息级别判断"要不要 recall"，而是维护会话级的 working memory 状态。

## 认知科学基础

| 概念 | 来源 | 应用 |
|------|------|------|
| 容量限制 | Miller's Law (7±2 chunks) | Working memory 最多存 7 条活跃记忆 |
| 时间衰减 | Baddeley's Working Memory Model | 5分钟无访问则淡出 |
| Spreading Activation | ACT-R | 当前 working memory 作为 context boost |
| Hebbian Neighborhood | Hebbian Learning | 相关话题在邻域内则不触发新 recall |

## 实现设计

```python
class SessionWorkingMemory:
    """模拟认知科学的 working memory — 有限容量 + 时间衰减"""
    
    def __init__(self, capacity=7, decay_seconds=300):
        self.capacity = capacity  # Miller's Law: 7±2 chunks
        self.decay_seconds = decay_seconds  # 5分钟无访问则淡出
        self.items: dict[str, float] = {}  # memory_id -> last_activated
    
    def activate(self, memory_ids: list[str]):
        """新 recall 的记忆进入 working memory"""
        now = time.time()
        for mid in memory_ids:
            self.items[mid] = now
        self._prune()
    
    def _prune(self):
        """移除衰减的和超容量的"""
        now = time.time()
        # 时间衰减
        self.items = {k: v for k, v in self.items.items() 
                      if now - v < self.decay_seconds}
        # 容量限制 (保留最近的)
        if len(self.items) > self.capacity:
            sorted_items = sorted(self.items.items(), key=lambda x: -x[1])
            self.items = dict(sorted_items[:self.capacity])
    
    def needs_recall(self, message: str, engram: Memory) -> bool:
        """判断是否需要新 recall"""
        self._prune()
        
        if not self.items:
            return True
        
        current_ids = list(self.items.keys())
        neighbors = set()
        for mid in current_ids:
            for link in engram.hebbian_links(mid):
                neighbors.add(link[1])  # target_id
        
        # 轻量 probe
        probe = engram.recall(message, limit=3)
        probe_ids = {r['id'] for r in probe}
        
        # 检查重叠
        overlap = probe_ids & (set(current_ids) | neighbors)
        if len(overlap) >= len(probe_ids) * 0.6:
            return False  # 话题连续
        
        return True  # 话题切换
```

## API 设计

### Memory 类新增方法

```python
class Memory:
    def session_recall(self, message: str, session_wm: SessionWorkingMemory = None) -> list[dict]:
        """智能 recall — 只在需要时执行完整检索"""
        if session_wm is None:
            return self.recall(message)  # fallback 到普通 recall
        
        if session_wm.needs_recall(message, self):
            results = self.recall(message, limit=5)
            session_wm.activate([r['id'] for r in results])
            return results
        else:
            # 返回 working memory 中的相关记忆
            return self._get_wm_context(session_wm)
```

### MCP 工具新增

```python
@mcp.tool(name="session_recall")
def session_recall(query: str, session_id: str = "default") -> list[dict]:
    """Session-aware recall — 自动判断是否需要完整检索"""
    ...
```

## 成本对比

| 场景 | 每条都 recall | Session WM |
|------|--------------|------------|
| 连续对话 (10条) | 10 次完整 recall | 1-2 次完整 + 8-9 次跳过 |
| 话题切换 | 10 次完整 recall | 话题数 × 完整 recall |
| Token 消耗 | ~15000 tokens | ~3000-5000 tokens |

## 测试计划

1. 单元测试：容量限制、时间衰减
2. 集成测试：needs_recall 判断准确性
3. 对话模拟：连续话题 vs 切换话题的触发率
