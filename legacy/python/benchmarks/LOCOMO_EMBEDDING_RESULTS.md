# LoCoMo Benchmark Results - Embedding Comparison

*Generated: 2026-02-03T13:10:46.307684*

## Results

| Mode | MRR | Hit@5 | Avg Latency |
|------|-----|-------|-------------|
| FTS5-only | 0.023 | 3.2% | 4.1ms |
| Embedding + ACT-R | 0.035 | 5.5% | 51.7ms |

## Improvement

- **MRR**: +48% (FTS5 → Embedding+ACT-R)
- **Hit@5**: +69%

## Interpretation

The embedding adapter provides semantic matching that FTS5 cannot do.
ACT-R then applies temporal reasoning (recency, frequency, importance) to rank candidates.

Combined with our Temporal Dynamics Benchmark results:
- LoCoMo (semantic retrieval): Embedding+ACT-R significantly outperforms FTS5
- TDB (temporal reasoning): ACT-R achieves 80% vs 20% for cosine-only

This validates the hybrid architecture: **embeddings find candidates, ACT-R decides priority**.
