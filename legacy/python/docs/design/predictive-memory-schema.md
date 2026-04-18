# Predictive Memory & Schema Formation

> 不等 query，主动推送 "你接下来可能需要这个记忆"

## 问题

当前 Engram 是 **被动检索**：agent 发 query → Engram 返回结果。

人脑不是这样的。你走进餐厅，大脑在你还没坐下之前就已经激活了：
- "上次这家的牛排很好吃"
- "服务员态度不好"
- "我对花生过敏，要提醒他们"

这是 **predictive activation** — 基于 context 的主动记忆推送，不需要显式 query。

## 认知科学基础

| 理论 | 来源 | 应用 |
|------|------|------|
| **Schema Theory** | Bartlett (1932), Rumelhart (1980) | 重复经验 → 抽象模板，新场景自动匹配 |
| **Predictive Processing** | Clark (2013), Friston (2010) | 大脑持续预测下一步，记忆是预测的原材料 |
| **Priming** | Meyer & Schvaneveldt (1971) | 先前刺激降低后续相关刺激的处理阈值 |
| **Situation Models** | Zwaan & Radvansky (1998) | 理解当前情境 → 预激活相关记忆 |
| **Event Segmentation** | Zacks & Tversky (2001) | 大脑自动把连续体验切分为离散"事件" |

## 两个子系统

### A. Schema Formation（模式提取）
### B. Predictive Activation（预测性推送）

---

## A. Schema Formation

### 什么是 Schema

Schema 是从重复经验中提取的 **抽象模板**：

```
经验 1: user 早上问天气 → 讨论上班路线 → 提到会议
经验 2: user 早上问新闻 → 讨论通勤 → 提到日程
经验 3: user 早上打招呼 → 讨论交通 → 提到 standup
                    ↓
Schema: "早上 session = 寒暄 → 通勤/天气 → 工作安排"
```

### Schema 数据结构

```python
@dataclass
class Schema:
    id: str
    name: str                          # 自动生成或 LLM 命名
    pattern: list[str]                 # 有序的 topic/action 序列
    trigger_context: dict              # 触发条件 (时间、关键词等)
    associated_memories: list[str]     # 相关 memory IDs
    occurrence_count: int              # 被匹配的次数
    confidence: float                  # 模式置信度
    created_at: datetime
    last_matched: datetime
```

```sql
CREATE TABLE schemas (
    id TEXT PRIMARY KEY,
    name TEXT,
    pattern TEXT NOT NULL,             -- JSON: ["greet", "commute", "work_planning"]
    trigger_context TEXT,              -- JSON: {"time_of_day": "morning", "keywords": [...]}
    associated_memories TEXT,          -- JSON: [memory_ids]
    occurrence_count INTEGER DEFAULT 1,
    confidence REAL DEFAULT 0.3,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_matched TIMESTAMP
);

-- Schema-memory associations
CREATE TABLE schema_memories (
    schema_id TEXT REFERENCES schemas(id),
    memory_id TEXT REFERENCES memories(id),
    position INTEGER,                  -- 在 pattern 中的位置
    PRIMARY KEY (schema_id, memory_id)
);
```

### Schema 提取算法

```python
class SchemaExtractor:
    """从 session 历史中提取重复模式"""
    
    def __init__(self, min_occurrences: int = 3, min_confidence: float = 0.5):
        self.min_occurrences = min_occurrences
        self.min_confidence = min_confidence
    
    def extract_from_sessions(self, session_logs: list[SessionLog]) -> list[Schema]:
        """
        从多个 session 的 recall 历史中找模式
        
        方法：Sequential Pattern Mining (PrefixSpan 的简化版)
        - 每个 session 的 recall 序列 = 一个 transaction
        - 找频繁子序列 = schema candidates
        """
        
        # 1. 把每个 session 的 recall 转成 topic 序列
        sequences = []
        for session in session_logs:
            topics = self._extract_topics(session.recalls)
            sequences.append(topics)
        
        # 2. 找频繁子序列 (长度 ≥ 2)
        frequent = self._find_frequent_subsequences(
            sequences, 
            min_support=self.min_occurrences
        )
        
        # 3. 提取 context 特征 (时间、关键词)
        schemas = []
        for pattern, support in frequent:
            context = self._extract_trigger_context(pattern, session_logs)
            schema = Schema(
                id=generate_id(),
                name=self._name_schema(pattern),
                pattern=pattern,
                trigger_context=context,
                occurrence_count=support,
                confidence=support / len(session_logs),
            )
            schemas.append(schema)
        
        return schemas
    
    def _extract_topics(self, recalls: list[RecallEvent]) -> list[str]:
        """把 recall 事件序列转成 topic 序列"""
        topics = []
        for recall in recalls:
            # 用 Hebbian cluster 或 FTS 关键词来归类
            topic = self._classify_topic(recall.query, recall.results)
            if topic != topics[-1] if topics else True:  # 去重连续相同
                topics.append(topic)
        return topics
    
    def _find_frequent_subsequences(
        self, sequences: list[list[str]], min_support: int
    ) -> list[tuple[list[str], int]]:
        """简化版 PrefixSpan — 找频繁有序子序列"""
        from collections import Counter
        
        # 找所有长度 2-4 的子序列
        candidates = Counter()
        for seq in sequences:
            seen = set()
            for length in range(2, min(5, len(seq) + 1)):
                for i in range(len(seq) - length + 1):
                    subseq = tuple(seq[i:i+length])
                    if subseq not in seen:
                        candidates[subseq] += 1
                        seen.add(subseq)
        
        return [
            (list(subseq), count) 
            for subseq, count in candidates.items()
            if count >= min_support
        ]
    
    def _extract_trigger_context(
        self, pattern: list[str], sessions: list[SessionLog]
    ) -> dict:
        """从匹配 pattern 的 sessions 中提取 context 特征"""
        matching_sessions = [s for s in sessions if self._matches(pattern, s)]
        
        context = {}
        
        # 时间特征
        hours = [s.start_time.hour for s in matching_sessions]
        if len(set(hours)) <= 3:  # 集中在少数几个小时
            avg_hour = sum(hours) / len(hours)
            context["time_of_day"] = (
                "morning" if avg_hour < 12
                else "afternoon" if avg_hour < 17
                else "evening"
            )
        
        # 星期特征
        days = [s.start_time.weekday() for s in matching_sessions]
        if all(d < 5 for d in days):
            context["day_type"] = "weekday"
        elif all(d >= 5 for d in days):
            context["day_type"] = "weekend"
        
        return context
```

### Schema 不依赖 LLM

**Topic 分类**用现有基础设施：
1. **Hebbian clusters** — 经常 co-activate 的记忆已经形成了自然 clusters = topics
2. **FTS5 关键词** — 每条 recall 的 query 词 = topic signal
3. **Memory type** — episodic vs semantic vs procedural 本身就是分类

---

## B. Predictive Activation

### 流程

```
Session 开始 / 新消息到达
    │
    ▼
┌──────────────────────────────────────────┐
│ 1. CONTEXT EXTRACTION                     │
│    当前时间、最近 N 条消息、活跃记忆       │
└────────────────────┬─────────────────────┘
                     │
                     ▼
┌──────────────────────────────────────────┐
│ 2. SCHEMA MATCHING                        │
│    当前 context 匹配哪些 schema？          │
│    ├─ 时间匹配 (morning session?)          │
│    ├─ 话题匹配 (当前 topic = schema[0]?)   │
│    └─ 序列位置 (schema 进行到哪一步?)      │
└────────────────────┬─────────────────────┘
                     │ matched
                     ▼
┌──────────────────────────────────────────┐
│ 3. PREDICTIVE RECALL                      │
│    预激活 schema 后续步骤的关联记忆         │
│    ├─ schema.pattern[next_step] 的记忆     │
│    ├─ 给这些记忆一个 prediction_boost       │
│    └─ 标记为 "predictive" (区别于 query)   │
└────────────────────┬─────────────────────┘
                     │
                     ▼
┌──────────────────────────────────────────┐
│ 4. DELIVERY                               │
│    ├─ Passive: 加到 recall 结果里           │
│    │   (当 agent 下次 query 时自然排高)     │
│    └─ Active: 主动推送给 agent              │
│        (通过 callback / event)              │
└──────────────────────────────────────────┘
```

### API 设计

```python
class Memory:
    def predict(self, context: dict = None) -> list[PredictedMemory]:
        """根据当前 context 预测可能需要的记忆"""
        
        if context is None:
            context = self._build_current_context()
        
        predictions = []
        
        # 1. Schema-based prediction
        matched_schemas = self._match_schemas(context)
        for schema, position in matched_schemas:
            # 获取 schema 后续步骤的关联记忆
            next_memories = self._get_schema_step_memories(
                schema, position + 1
            )
            for mem in next_memories:
                predictions.append(PredictedMemory(
                    memory=mem,
                    reason=f"schema:{schema.name}",
                    confidence=schema.confidence,
                    predicted_step=position + 1,
                ))
        
        # 2. Temporal prediction (同一时间段常用的记忆)
        temporal = self._temporal_predictions(context.get("time"))
        predictions.extend(temporal)
        
        # 3. Priming (最近 recall 的 Hebbian 邻居)
        if context.get("recent_recalls"):
            primed = self._priming_predictions(context["recent_recalls"])
            predictions.extend(primed)
        
        # 去重 + 按 confidence 排序
        return self._deduplicate_predictions(predictions)
    
    def _build_current_context(self) -> dict:
        """构建当前 context"""
        return {
            "time": datetime.now(),
            "day_of_week": datetime.now().weekday(),
            "recent_recalls": self._session_wm.items if self._session_wm else [],
            "active_topics": self._get_active_topics(),
        }
    
    def _match_schemas(self, context: dict) -> list[tuple[Schema, int]]:
        """匹配当前 context 的 schemas + 当前进行到的位置"""
        schemas = self._store.get_schemas()
        matches = []
        
        for schema in schemas:
            # 时间匹配
            time_match = self._check_time_match(
                schema.trigger_context, context
            )
            
            # 序列位置检测
            position = self._detect_schema_position(schema, context)
            
            if time_match and position >= 0:
                matches.append((schema, position))
        
        return sorted(matches, key=lambda x: x[0].confidence, reverse=True)
    
    def on_session_start(self, callback=None):
        """Session 开始时的预测性推送"""
        predictions = self.predict()
        if predictions and callback:
            callback(predictions)
        return predictions
    
    def on_topic_change(self, new_topic: str, callback=None):
        """话题切换时的预测性推送"""
        context = self._build_current_context()
        context["current_topic"] = new_topic
        predictions = self.predict(context)
        if predictions and callback:
            callback(predictions)
        return predictions


@dataclass
class PredictedMemory:
    memory: MemoryEntry
    reason: str              # "schema:morning_routine" | "temporal" | "priming"
    confidence: float        # 0-1
    predicted_step: int = 0  # schema 中的预测步骤
```

### Priming（短期预测）

基于 Hebbian 邻居的即时预测 — 不需要 schema：

```python
def _priming_predictions(self, recent_recall_ids: list[str]) -> list[PredictedMemory]:
    """最近 recall 的 Hebbian 邻居 = 可能马上需要的记忆"""
    predictions = []
    
    for recall_id in recent_recall_ids:
        neighbors = self._store.get_hebbian_neighbors(
            recall_id, 
            min_strength=0.5,  # 只取强链接
            limit=3
        )
        for neighbor in neighbors:
            if neighbor.id not in recent_recall_ids:  # 排除已在 WM 中的
                predictions.append(PredictedMemory(
                    memory=neighbor,
                    reason="priming",
                    confidence=neighbor.hebbian_strength * 0.8,
                ))
    
    return predictions
```

**示例**：agent 刚 recall 了 "user prefers Docker"，Hebbian 邻居 "user uses docker-compose for staging" 被 primed → 如果 agent 接下来问部署相关的问题，这条记忆直接排到前面。

### Temporal Prediction（时间模式）

```python
def _temporal_predictions(self, current_time: datetime) -> list[PredictedMemory]:
    """基于历史 access 时间模式的预测"""
    
    hour = current_time.hour
    day = current_time.weekday()
    
    # 查询在类似时间段被频繁访问的记忆
    # 但最近一段时间没被访问的（避免重复推送）
    candidates = self._store.get_temporally_correlated(
        hour_range=(hour - 1, hour + 1),
        day_type="weekday" if day < 5 else "weekend",
        min_temporal_access=3,     # 至少在这个时段被访问过 3 次
        not_accessed_hours=2,      # 最近 2 小时没被访问
    )
    
    return [
        PredictedMemory(
            memory=mem,
            reason="temporal",
            confidence=mem.temporal_correlation,
        )
        for mem in candidates[:5]
    ]
```

## 与现有系统的集成

### 在 recall() 中融入预测

```python
class Memory:
    def recall(self, query: str, limit: int = 5, 
               include_predictions: bool = True) -> list[dict]:
        """扩展 recall — 可选融入预测结果"""
        
        # 原有 recall 逻辑
        results = self._search_and_rank(query, limit)
        
        if include_predictions:
            predictions = self.predict()
            for pred in predictions:
                # 给预测记忆一个 activation boost
                # （不改变原始 activation，只在本次 recall 中临时加分）
                pred.memory._temp_boost = pred.confidence * PREDICTION_WEIGHT
            
            # 重新排序
            all_candidates = results + [p.memory for p in predictions]
            results = self._rerank(all_candidates, limit)
        
        return results
```

### 在 consolidate() 中更新 schema

```python
class Memory:
    def consolidate(self, distill=True, update_schemas=True, llm=None):
        """扩展 consolidate — 加入 schema 更新"""
        
        # 原有逻辑
        super().consolidate()
        
        if distill:
            self._distill_episodic_to_semantic(llm)
        
        if update_schemas:
            # 从最近的 session logs 中提取/更新 schemas
            recent_sessions = self._store.get_recent_sessions(days=30)
            extractor = SchemaExtractor()
            new_schemas = extractor.extract_from_sessions(recent_sessions)
            
            for schema in new_schemas:
                existing = self._store.find_similar_schema(schema)
                if existing:
                    existing.occurrence_count += schema.occurrence_count
                    existing.confidence = min(0.95, existing.confidence + 0.1)
                    self._store.update_schema(existing)
                else:
                    self._store.add_schema(schema)
```

## 参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `SCHEMA_MIN_OCCURRENCES` | 3 | 模式至少出现多少次才提取 |
| `SCHEMA_MIN_CONFIDENCE` | 0.3 | schema 最低置信度 |
| `SCHEMA_MAX_LENGTH` | 5 | schema 最大步骤数 |
| `PREDICTION_WEIGHT` | 0.3 | 预测记忆的 activation boost 系数 |
| `PRIMING_MIN_STRENGTH` | 0.5 | Priming 的 Hebbian link 最低强度 |
| `TEMPORAL_MIN_ACCESS` | 3 | 时间模式需要的最少 access 次数 |
| `MAX_PREDICTIONS` | 5 | 每次最多推送几条预测 |

## 测试计划

```python
def test_schema_extraction():
    """重复的 session 模式 → schema 形成"""
    # 模拟 5 个类似结构的 session
    # 验证 extract_from_sessions 找到 pattern

def test_schema_matching():
    """当前 context 匹配已有 schema"""
    # 创建 schema (morning: greet → commute → work)
    # 设置 context = morning + greet 阶段
    # 验证 match 返回 position=0

def test_predictive_recall():
    """schema 匹配后 predict() 返回下一步记忆"""
    # 匹配到 morning schema position=1
    # predict() 应返回 position=2 (work planning) 相关记忆

def test_priming():
    """Hebbian 邻居被 primed"""
    # recall memory A → A 的 Hebbian 邻居 B 出现在 predict() 中
    
def test_temporal_prediction():
    """时间模式触发预测"""
    # 在 9AM 频繁访问 memory X
    # 模拟 9AM context → predict() 包含 X

def test_prediction_in_recall():
    """include_predictions=True 影响 recall 排序"""
    # 有预测 boost 的记忆排名高于无 boost 的相似记忆

def test_schema_decay():
    """长期不匹配的 schema 衰减"""
    # 30 天没匹配 → confidence 下降
```

## 实现优先级

```
Phase 1: Priming（最简单，最大 ROI）
    └── recall 时自动拉入 Hebbian 邻居
    └── 不需要新的数据结构
    └── 预计 2-3 天实现

Phase 2: Temporal Prediction
    └── 需要在 access_log 上加时间分析
    └── 简单统计，不需要 ML
    └── 预计 3-5 天实现

Phase 3: Schema Formation
    └── 需要 session log 基础设施
    └── 需要 pattern mining 算法
    └── 完整实现预计 1-2 周

Phase 4: Active Push
    └── 需要 callback/event 机制
    └── 与 agent framework 集成
    └── 预计 1 周
```

---

*Document created: 2026-03-09*
*Cognitive basis: Schema Theory (Bartlett 1932), Predictive Processing (Clark 2013), Priming (Meyer & Schvaneveldt 1971)*
