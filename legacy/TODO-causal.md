# TODO: Causal Memory Upgrade

> See full design: `/Users/potato/clawd/projects/causal-agent/DESIGN.md`
> See Hebbian design: `docs/design/hebbian-learning.md` (Section 2: STDP)

## Phase 1: `type=causal` + metadata (方案 B)

**Priority: High | Complexity: Low**

### Changes needed:

1. **Add `type=causal`** to recognized memory types
   - File: `engram/core.py` (or wherever types are validated)
   - Just add "causal" to the allowed types list

2. **Add `metadata` field to `store()`**
   - Optional JSON object parameter
   - Schema for causal memories:
     ```json
     {
       "cause": "description of cause",
       "effect": "description of effect",
       "outcome": "positive | negative | neutral",
       "domain": "code | ops | trading | research | diagnostic",
       "context": "optional reference (e.g. django__django-14382)"
     }
     ```
   - Store in a `metadata` column (JSON text) in the memories table

3. **Add `recall_causal(cause_query)` convenience method**
   - Filters by `type=causal`
   - Semantic search on the `cause` field in metadata
   - Returns causal memories ranked by activation + relevance

4. **MCP tool update**
   - Add `metadata` parameter to `engram.store` tool
   - Add `engram.recall_causal` tool

### SQL migration:
```sql
ALTER TABLE memories ADD COLUMN metadata TEXT;  -- JSON
CREATE INDEX idx_memories_type ON memories(type);
```

---

## Phase 2: Hebbian STDP Upgrade (因果方向自动发现)

**Priority: Medium | Complexity: Medium**

### What it does:
During `consolidate()`, automatically detect causal direction from temporal ordering of co-activated memory pairs. If memory A consistently precedes memory B, create a causal link A→B.

### Changes needed:

1. **Add temporal tracking to hebbian_links table**
   ```sql
   ALTER TABLE hebbian_links ADD COLUMN direction TEXT DEFAULT 'bidirectional';
   ALTER TABLE hebbian_links ADD COLUMN temporal_forward INTEGER DEFAULT 0;
   ALTER TABLE hebbian_links ADD COLUMN temporal_backward INTEGER DEFAULT 0;
   ```

2. **Track temporal ordering during co-activation counting**
   - When memories {m1, m2, ...mk} co-activate in a recall
   - For each pair (mi, mj), check `created_at` timestamps
   - Increment `temporal_forward` if mi was stored before mj
   - Increment `temporal_backward` if mj was stored before mi

3. **Auto-create causal memories during `consolidate()`**
   - If `temporal_forward > temporal_backward * 2` → create A→B causal memory
   - Use `type=causal` with metadata from Phase 1
   - Confidence = `forward / (forward + backward)`

4. **Update link direction**
   - Change symmetric link to directional when temporal signal is strong
   - Affects spreading activation (A→B propagates, B→A doesn't)

### Full design: `docs/design/hebbian-learning.md` Section 2 (STDP)

---

## Integration Points

- **SWE-bench agent**: First consumer of causal memories
  - Store: "changing X → test Y failed" after each test run
  - Recall: "what happens when I change functions like X?" before patch gen
  
- **GID**: Two-layer architecture
  - GID = structural causation (static, from code analysis)
  - Engram STDP = experiential causation (dynamic, from observations)
  - Both agree → high confidence prediction
  - Disagree → investigate further

- **botcore CausalWorkflow**: Uses `recall_causal()` in Decide phase
