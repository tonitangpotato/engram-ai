# Temporal Dynamics Benchmark - Baseline Comparison

*Generated: 2026-02-03*

## What This Tests

The Temporal Dynamics Benchmark tests **temporal reasoning** — knowing which memory is *current*, not just *relevant*.

| Category | Description | What ACT-R Brings |
|----------|-------------|-------------------|
| **recency_override** | Newer info should replace older | Forgetting curve decays old memories |
| **frequency** | Repeated mentions should rank higher | Hebbian strengthening through access |
| **importance** | Critical info persists despite age | Importance weights in activation |
| **contradiction** | Latest state wins in conflicts | Temporal decay + contradiction penalty |

## Results

| System | recency | frequency | importance | contradiction | Overall |
|--------|---------|-----------|------------|---------------|---------|
| **engram (ACT-R)** | **100.0%** | **100.0%** | **100.0%** | **100.0%** | **100.0%** |
| Recency-Only | 20.0% | 18.0% | 20.0% | 20.0% | 19.5% |
| Cosine-Only (Jaccard proxy) | 0.0% | 18.0% | 50.0% | 20.0% | 22.0% |
| Random | 8.0% | 18.0% | 38.0% | 20.0% | 21.0% |

## Key Improvements (v2)

### Optimizations Applied

1. **Proper timestamp simulation** — `created_at` parameter allows benchmarks to simulate temporal order
2. **Contradiction penalty** — Memories marked as contradicted receive -3.0 activation penalty
3. **Balanced importance weighting** — `importance_weight` increased to 2.0 to properly compete with recency

### engram (ACT-R) vs Baselines

1. **Frequency reasoning**: engram 100% vs Cosine-Only 18% (+82%)
   - ACT-R's Hebbian strengthening makes frequently-accessed memories more available
   
2. **Importance persistence**: engram 100% vs Cosine-Only 50% (+50%)
   - Important memories resist decay even when older
   
3. **Recency override**: engram 100% vs all baselines <25%
   - Proper temporal decay ensures newer information wins

4. **Contradiction handling**: engram 100% vs baselines ~20%
   - Contradiction penalty ensures superseded information doesn't surface

### The ACT-R Advantage

Pure cosine similarity treats all memories equally — a mention of "pizza" from day 1 and day 15 have the same weight if the query matches both.

ACT-R activation considers:
- **Recency**: Recent memories are more accessible (power law decay)
- **Frequency**: Repeatedly accessed memories are stronger (base-level activation)
- **Importance**: Critical memories persist (importance boost)
- **Contradiction**: Superseded memories are penalized (contradiction penalty)
- **Spreading activation**: Associated memories prime each other

This is how human memory works. It's why you remember your current job, not your first job, when asked "where do you work?"
