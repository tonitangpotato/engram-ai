# Benchmark Coverage

*Which benchmarks test which cognitive mechanisms*

## Current Benchmarks

### 1. Temporal Dynamics Benchmark (TDB)
**Location:** `benchmarks/temporal_benchmark.py`

| Category | Tests | engram Score | Baseline |
|----------|-------|--------------|----------|
| recency_override | Newer info > older | 100% | 20% |
| frequency | Repeated mentions | 100% | 18% |
| importance | Critical info persists | 100% | 50% |
| contradiction | Latest state wins | 100% | 20% |

**Mechanisms Tested:**
- ✅ ACT-R base activation (recency + frequency)
- ✅ Importance weighting
- ✅ Contradiction detection

---

### 2. Repeated Access Benchmark (RAB)
**Location:** `benchmarks/repeated_access_benchmark.py`

| Category | Tests | engram Score | Baseline |
|----------|-------|--------------|----------|
| frequency | Access count boosting | 62.5% | 87.5% |
| hebbian | Co-activation links | 66.7% | 33.3% |
| combined | Importance + frequency | 100% | 50% |

**Mechanisms Tested:**
- ✅ ACT-R access counting
- ✅ Hebbian learning
- ✅ Importance + frequency interaction

**Note:** Baseline beats engram on frequency due to keyword matching dominance. ACT-R wins on hebbian and combined.

---

### 3. LoCoMo Embedding Ablation
**Location:** `benchmarks/ablation_embedding.py`

| Config | MRR | Hit@5 |
|--------|-----|-------|
| embedding-only | 0.254 | 0.384 |
| keyword-only | 0.221 | 0.291 |
| embedding+actr | 0.210 | 0.318 |

**Mechanisms Tested:**
- ✅ Embedding vs keyword retrieval
- ⚠️ ACT-R (hurts in static benchmark)

**Finding:** ACT-R doesn't help static retrieval. Its value is in temporal/dynamic scenarios.

---

### 4. LoCoMo Retrieval Ablation  
**Location:** `benchmarks/ablation_retrieval.py`

Tests various ACT-R component combinations on retrieval metrics.

**Mechanisms Tested:**
- ✅ ACT-R components (ablation)
- ✅ Hebbian (in isolation)
- ✅ Decay (in isolation)

---

## Coverage Matrix

| Mechanism | TDB | RAB | LoCoMo | Dedicated Test |
|-----------|-----|-----|--------|----------------|
| ACT-R Base Activation | ✅ | ✅ | ⚠️ | ✅ |
| ACT-R Spreading | ❌ | ❌ | ❌ | ❌ **NEEDED** |
| Hebbian Learning | ❌ | ✅ | ❌ | ✅ |
| Ebbinghaus Decay | ⚠️ | ❌ | ❌ | ❌ **NEEDED** |
| Contradiction/RIF | ✅ | ❌ | ❌ | ✅ |
| Importance | ✅ | ✅ | ❌ | ✅ |
| Consolidation | ❌ | ❌ | ❌ | ❌ **NEEDED** |
| Memory Types | ❌ | ❌ | ❌ | ❌ **NEEDED** |

---

## Needed Benchmarks

### 1. Spreading Activation Benchmark
```python
# Test: Query "X" should boost "Y" if they share context

memories = [
    "Project Alpha uses Python",
    "Python is great for data science", 
    "We discussed data science in the meeting",
]

query = "Tell me about Project Alpha"
# Expected: All three should surface due to spreading
#   Alpha → Python → data science → meeting
```

### 2. Decay Curve Benchmark
```python
# Test: Old memories have lower retrievability

# Add memory at t=0
mem.add("Important fact")

# Simulate time passage
# Check retrievability at t=1d, 7d, 30d, 90d
# Should follow Ebbinghaus curve
```

### 3. Consolidation Benchmark
```python
# Test: Consolidation improves long-term recall

# Add memories to working memory
# Run consolidation
# Check: strong memories promoted, weak demoted
# Check: Hebbian links decayed appropriately
```

### 4. Memory Type Decay Benchmark
```python
# Test: Different types decay at different rates

# Add EPISODIC and SEMANTIC memories at same time
# Wait (simulated)
# EPISODIC should have lower retrievability than SEMANTIC
```

---

## Running Benchmarks

```bash
# TDB (fast, no external deps)
python benchmarks/temporal_benchmark.py

# RAB (fast, no external deps)  
python benchmarks/repeated_access_benchmark.py

# LoCoMo with embeddings (requires sentence-transformers)
python benchmarks/ablation_embedding.py --limit 3

# LoCoMo retrieval metrics
python benchmarks/ablation_retrieval.py --limit 3
```

---

## Key Findings Summary

1. **ACT-R excels at temporal reasoning** (TDB: 100% vs 20% baseline)
2. **Embedding excels at semantic retrieval** (LoCoMo: +15% over keyword)
3. **Hebbian helps associative recall** (RAB: +33% over baseline)
4. **Combined mechanisms work best** (RAB combined: 100% vs 50%)

**Conclusion:** engram is not a replacement for embedding search. It's a **cognitive layer** that adds temporal/importance/associative reasoning on top of retrieval.
