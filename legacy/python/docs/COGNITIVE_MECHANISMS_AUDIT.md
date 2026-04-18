# Cognitive Mechanisms Audit

*Audit of cognitive science mechanisms in engram's search pipeline*

## Search Pipeline Overview

```
Query
  │
  ▼
┌─────────────────────────────────────────────────────────────┐
│ 1. CANDIDATE RETRIEVAL                                       │
│    ├─ Vector search (embedding similarity)   [if available]  │
│    └─ FTS5 keyword search                    [always]        │
├─────────────────────────────────────────────────────────────┤
│ 2. GRAPH EXPANSION                                           │
│    ├─ Entity-based expansion (1-hop)                         │
│    └─ Hebbian neighbor expansion                             │
├─────────────────────────────────────────────────────────────┤
│ 3. ACT-R SCORING                                             │
│    ├─ Base-level activation (frequency × recency)            │
│    ├─ Spreading activation (context keywords)                │
│    ├─ Importance boost                                       │
│    ├─ Contradiction penalty                                  │
│    └─ Hebbian spreading boost                                │
├─────────────────────────────────────────────────────────────┤
│ 4. CONFIDENCE SCORING                                        │
│    └─ Ebbinghaus retrievability + strength                   │
├─────────────────────────────────────────────────────────────┤
│ 5. RANKING & FILTERING                                       │
│    ├─ Sort by combined score                                 │
│    └─ Apply min_confidence filter                            │
└─────────────────────────────────────────────────────────────┘
  │
  ▼
Results
```

---

## Mechanism Audit

### 1. ACT-R Base-Level Activation

| Aspect | Status | Details |
|--------|--------|---------|
| **Implementation** | ✅ Complete | `engram/activation.py:base_level_activation()` |
| **Formula** | ✅ Correct | `B_i = ln(Σ_k t_k^(-d))` with d=0.5 |
| **Access logging** | ✅ Fixed | `Memory.recall()` now calls `record_access()` |
| **Pipeline integration** | ✅ Used | `retrieval_activation()` called in scoring |

**Benchmark coverage:**
- ✅ TDB: recency_override tests
- ✅ RAB: frequency tests

**Agent scenarios:**
- ✅ Frequently discussed topics rank higher
- ✅ Recently mentioned info is more accessible

---

### 2. Spreading Activation

| Aspect | Status | Details |
|--------|--------|---------|
| **Implementation** | ✅ Complete | `engram/activation.py:spreading_activation()` |
| **Pipeline integration** | ⚠️ Partial | Only used if `context_keywords` provided |

**Issue:** Context keywords must be explicitly passed. Most callers don't provide them.

**Recommendation:** Auto-extract keywords from query.

```python
# Current (requires explicit keywords):
engine.search(query="...", context_keywords=["potato", "project"])

# Better (auto-extract):
def extract_keywords(query: str) -> list[str]:
    # Remove stop words, return significant terms
    ...
```

**Benchmark coverage:**
- ❌ No dedicated benchmark

**Agent scenarios:**
- Query "project" should boost memories containing "potato" if they co-occur often

---

### 3. Hebbian Learning

| Aspect | Status | Details |
|--------|--------|---------|
| **Co-activation recording** | ✅ Complete | `hebbian.py:record_coactivation()` |
| **Link formation** | ✅ Complete | Threshold-based (default=3) |
| **Link strengthening** | ✅ Complete | Repeated co-access increases strength |
| **Link decay** | ✅ Complete | `decay_hebbian_links()` in consolidation |
| **Spreading boost** | ✅ Complete | `_score_candidates()` adds Hebbian boost |
| **Graph expansion** | ✅ Complete | `_expand_via_graph()` includes Hebbian neighbors |

**Benchmark coverage:**
- ✅ RAB: hebbian tests (66.7% accuracy)

**Agent scenarios:**
- ✅ "Coffee" and "morning" get linked through repeated co-retrieval
- ✅ Linked memories boost each other's retrieval

---

### 4. Ebbinghaus Forgetting Curve

| Aspect | Status | Details |
|--------|--------|---------|
| **Retrievability** | ✅ Complete | `forgetting.py:retrievability()` |
| **Stability growth** | ✅ Complete | `compute_stability()` with spacing effect |
| **Importance modulation** | ✅ Complete | High importance = higher stability |
| **Pipeline integration** | ✅ Used | Via `confidence_score()` |

**Issue:** Retrievability affects *confidence*, not ranking directly.

**Recommendation:** Consider using retrievability in ranking too.

**Benchmark coverage:**
- ⚠️ Partially in TDB (implicit via recency)

**Agent scenarios:**
- ✅ Old memories have lower confidence
- ✅ Frequently accessed memories maintain retrievability

---

### 5. Contradiction Detection (Retrieval-Induced Forgetting)

| Aspect | Status | Details |
|--------|--------|---------|
| **Contradiction marking** | ✅ Complete | `entry.contradicted_by` field |
| **Activation penalty** | ✅ Complete | `-3.0` penalty in `retrieval_activation()` |
| **Retrieval suppression** | ✅ Complete | `retrieval_induced_forgetting()` |

**Benchmark coverage:**
- ✅ TDB: contradiction tests (100% accuracy)

**Agent scenarios:**
- ✅ "I live in Seattle" suppresses old "I live in SF"

---

### 6. Importance Weighting

| Aspect | Status | Details |
|--------|--------|---------|
| **Storage** | ✅ Complete | `entry.importance` (0-1) |
| **Activation boost** | ✅ Complete | `importance * importance_weight` |
| **Stability boost** | ✅ Complete | In `compute_stability()` |

**Benchmark coverage:**
- ✅ TDB: importance tests (100% accuracy)
- ✅ RAB: combined tests (100% accuracy)

**Agent scenarios:**
- ✅ Allergy info persists despite age
- ✅ Important deadlines don't get buried

---

### 7. Consolidation (Sleep Replay)

| Aspect | Status | Details |
|--------|--------|---------|
| **Working→Core transfer** | ✅ Complete | `consolidation.py` |
| **Synaptic downscaling** | ✅ Complete | `synaptic_downscale()` |
| **Hebbian decay** | ✅ Complete | `decay_hebbian_links()` |
| **Layer promotion/demotion** | ✅ Complete | Threshold-based |

**Issue:** Must be called manually (`mem.consolidate()`). No automatic scheduling.

**Recommendation:** Consider automatic consolidation in long-running agents.

**Benchmark coverage:**
- ❌ No dedicated benchmark

**Agent scenarios:**
- Session boundaries could trigger consolidation
- Important info gets promoted to core strength

---

### 8. Memory Types & Decay Rates

| Aspect | Status | Details |
|--------|--------|---------|
| **Type definitions** | ✅ Complete | EPISODIC, SEMANTIC, PROCEDURAL, etc. |
| **Decay rates** | ✅ Complete | Type-specific in `DEFAULT_DECAY_RATES` |
| **Layer filtering** | ✅ Complete | L1_WORKING through L4_ARCHIVE |

**Benchmark coverage:**
- ❌ No benchmark tests type-specific decay

**Agent scenarios:**
- Facts (semantic) decay slower than events (episodic)
- Skills (procedural) persist longest

---

## Gap Analysis

### Missing in Pipeline

| Gap | Impact | Fix Effort |
|-----|--------|------------|
| Auto keyword extraction | Spreading activation underused | Low |
| Query type detection | Wrong retrieval strategy | Medium |
| Automatic consolidation | Memory growth unbounded | Medium |

### Missing Benchmarks

| Mechanism | Current Coverage | Needed |
|-----------|-----------------|--------|
| Spreading activation | ❌ None | Context-based retrieval test |
| Consolidation | ❌ None | Pre/post consolidation comparison |
| Memory types | ❌ None | Type-specific decay test |
| Retrievability | ⚠️ Implicit | Explicit "aged memory" test |

---

## Recommendations

### P0 (Do Now)

1. **Add auto keyword extraction** to enable spreading activation
2. **Create spreading activation benchmark**

### P1 (Soon)

3. **Add temporal query detection** in hybrid search (partially done)
4. **Benchmark consolidation** effects

### P2 (Later)

5. **Auto-consolidation** for long-running agents
6. **Memory type benchmark** for decay rates

---

## Summary Table

| Mechanism | Implemented | In Pipeline | Benchmarked | Agent Value |
|-----------|-------------|-------------|-------------|-------------|
| ACT-R Base Activation | ✅ | ✅ | ✅ | High |
| Spreading Activation | ✅ | ⚠️ Partial | ❌ | Medium |
| Hebbian Learning | ✅ | ✅ | ✅ | High |
| Ebbinghaus Decay | ✅ | ✅ | ⚠️ Implicit | Medium |
| Contradiction/RIF | ✅ | ✅ | ✅ | High |
| Importance | ✅ | ✅ | ✅ | High |
| Consolidation | ✅ | Manual | ❌ | Medium |
| Memory Types | ✅ | ✅ | ❌ | Low |
