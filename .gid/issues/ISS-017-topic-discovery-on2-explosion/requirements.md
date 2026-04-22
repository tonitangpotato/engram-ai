# Requirements: ISS-017 TopicDiscovery O(n²) Fix

## Overview

Replace the all-pairs O(n²) similarity computation in `TopicDiscovery::discover()` with an Approximate Nearest Neighbors (ANN) approach that scales to 100k+ memories while preserving cluster quality.

## Priority Levels

- **P0**: Core — required for the system to function at all
- **P1**: Important — needed for production-quality operation
- **P2**: Enhancement — improves efficiency, UX, or observability

## Goals

### GOAL-17.1: ANN-Based Graph Construction (P0)
Replace the O(n²) pairwise cosine similarity loop with HNSW-based approximate nearest neighbor search. Each memory finds its top-K most similar neighbors (K configurable, default 20) and only those edges are added to the Infomap network.

**Acceptance Criteria:**
- Graph construction time for 16k memories × 64-dim embeddings < 5 seconds
- Memory usage during graph construction < 200MB
- Each node has at most K outgoing edges (graph is guaranteed sparse)
- Total complexity is O(n·log n) for index build + O(n·K·log n) for queries

### GOAL-17.2: Configurable K and Threshold (P1)
The top-K value and minimum similarity threshold are configurable via `TopicDiscovery` builder methods, with sensible defaults.

**Acceptance Criteria:**
- `TopicDiscovery::with_top_k(k: usize)` sets max neighbors per node (default: 20)
- `edge_threshold` remains configurable (raise default from 0.3 to 0.4)
- If a node has fewer than K neighbors above threshold, it gets fewer edges (not padded)

### GOAL-17.3: Cluster Quality Preservation (P0)
The ANN-based approach must produce clusters of equivalent or better quality compared to the exact O(n²) approach on the same data.

**Acceptance Criteria:**
- On a synthetic test with 2 well-separated clusters: both approaches find the same clusters
- On a test with 3+ overlapping clusters: ANN produces coherent communities (cohesion > 0.5)
- No regression in existing `test_discover_basic_two_clusters` and related tests

### GOAL-17.4: Graceful Scaling (P1)
The system handles edge cases without panicking or hanging.

**Acceptance Criteria:**
- 0 memories → returns empty vec (no panic)
- 1 memory → returns empty vec (no panic)
- 2 memories → works correctly (degenerate case)
- 100k memories → completes in < 30 seconds
- If ANN index build fails for any reason → fallback to exact computation with hard cap (max 2000 memories, sampled)

### GOAL-17.5: Infomap Configuration Tightening (P2)
Reduce Infomap computational budget for the now-sparse graph.

**Acceptance Criteria:**
- Trials reduced from default (10) to 3 (sparse graphs converge faster)
- Total Infomap runtime for 16k-node sparse graph (320k edges max) < 10 seconds
- Seed remains fixed (42) for reproducibility

## Guards

### GUARD-1: No New Unsafe Code (hard)
The ANN implementation must be safe Rust. The chosen crate must not introduce unsound `unsafe` blocks.

### GUARD-2: API Backward Compatibility (hard)
`TopicDiscovery::discover(&self, memories: &[(String, Vec<f32>)])` signature must not change. Existing callers continue to work without modification.

### GUARD-3: Determinism (soft)
Given the same inputs and seed, results should be reproducible. ANN is approximate so exact bit-for-bit reproducibility is not required, but cluster membership should be stable across runs.

## Out of Scope

- Incremental/streaming topic discovery (future ISS)
- Persisting the ANN index between compilations (future optimization)
- Changing the embedding dimensionality or provider
- Modifying Infomap algorithm internals
