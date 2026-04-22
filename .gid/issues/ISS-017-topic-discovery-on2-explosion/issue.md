# ISS-017: TopicDiscovery O(n²) Explosion Kills Process

## Status: Open
## Priority: P0 (Critical — causes OOM and infinite hang)
## Component: `src/compiler/discovery.rs`

## Problem

`TopicDiscovery::discover()` computes **all-pairs cosine similarity** to build the Infomap input graph. At 16,366 memories:

- Comparisons: n×(n-1)/2 = **133,923,945** (1.34 billion FLOPS for 64-dim vectors)
- With threshold 0.3 (very permissive): ~40 million edges survive
- Infomap on a 40M-edge dense graph → `local_moves` cannot converge
- Result: CPU 100%, memory 3.7GB, process hangs indefinitely

## Root Cause

```rust
// discovery.rs — the O(n²) loop
for i in 0..n {
    for j in (i+1)..n {
        let sim = cosine_similarity(&memories[i].1, &memories[j].1);
        if sim >= self.edge_threshold {  // 0.3 is too low
            network.add_edge(i, j, sim);
        }
    }
}
```

Two compounding problems:
1. **O(n²) pairwise comparison** — doesn't scale past ~2000 memories
2. **Low threshold (0.3)** — allows too many edges through, creating a dense graph that Infomap struggles with

## Impact

- RustClaw process repeatedly killed by OOM or hung at 100% CPU
- `KnowledgeCompileTool` is effectively unusable with real-world memory volumes
- Forces manual `kill -9` intervention

## Fix Strategy

Replace O(n²) all-pairs with **Approximate Nearest Neighbors (ANN)** using HNSW index:
- Build time: O(n·log n)
- Query time per node: O(log n)
- Each node connects to at most K neighbors (K=20)
- Total edges capped at n×K = predictable, sparse graph
- Infomap converges fast on sparse graphs

## References

- GID's clustering (`gid-core/src/infer/clustering.rs`) avoids this entirely by using structural edges (imports, calls) — naturally sparse
- For memory (no structural edges), ANN is the equivalent approach to manufacture sparsity
