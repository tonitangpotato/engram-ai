# Design: ISS-017 TopicDiscovery O(n²) → ANN Fix

## 1. Overview

Replace the brute-force all-pairs similarity computation with a two-phase approach:
1. **Build HNSW index** from all memory embeddings (O(n·log n))
2. **Query top-K neighbors** per memory (O(K·log n) per query, O(n·K·log n) total)
3. **Feed sparse graph** to Infomap (max n×K edges instead of n²/2)

Satisfies: GOAL-17.1, GOAL-17.2, GOAL-17.3, GOAL-17.4, GOAL-17.5

## 2. Crate Selection

**Choice: `instant-distance`** (v0.6+)
- Pure Rust, no unsafe, MIT licensed
- HNSW algorithm (Hierarchical Navigable Small World)
- Supports custom distance functions (we use cosine = 1 - dot product on normalized vectors)
- Zero external C dependencies
- Battle-tested in production search systems

Alternative considered: `hnsw_rs` — more features but uses unsafe internally.

## 3. Architecture Change

### 3.1 Before (O(n²))

```
memories[] → double loop → HashMap<(i,j), f64> → Network → Infomap
             O(n²)          ~40M entries           dense
```

### 3.2 After (O(n·log n))

```
memories[] → HNSW::build()  → per-node top-K query → Network → Infomap
             O(n·log n)       O(n·K·log n)           sparse (≤n×K edges)
```

### 3.3 Edge Construction Detail

```rust
// Pseudocode for the new approach
let hnsw = HnswBuilder::default()
    .ef_construction(100)  // build quality (higher = more accurate, slower build)
    .build(points);        // O(n·log n)

let mut network = Network::with_capacity(n);
network.ensure_capacity(n);

let mut sim_cache: HashMap<(usize, usize), f64> = HashMap::new();

for i in 0..n {
    // Query returns up to top_k nearest neighbors with distances
    let neighbors = hnsw.search(&points[i], top_k, ef_search=50);
    
    for (j, distance) in neighbors {
        let sim = 1.0 - distance;  // cosine distance → similarity
        if sim >= self.edge_threshold && i != j {
            let (lo, hi) = if i < j { (i, j) } else { (j, i) };
            if !sim_cache.contains_key(&(lo, hi)) {
                network.add_edge(i, j, sim);
                network.add_edge(j, i, sim);
                sim_cache.insert((lo, hi), sim);
            }
        }
    }
}
```

### 3.4 Parameters

| Parameter | Default | Rationale |
|-----------|---------|-----------|
| `top_k` | 20 | Enough to capture all relevant neighbors; Infomap handles the rest |
| `edge_threshold` | 0.4 (raised from 0.3) | Reduces noise edges; 0.3 was too permissive |
| `ef_construction` | 100 | Standard HNSW build quality |
| `ef_search` | 50 | Balanced recall vs speed for search |
| Infomap `num_trials` | 3 (reduced from 10) | Sparse graphs converge faster |
| Infomap `seed` | 42 | Reproducibility |

## 4. Fallback Strategy (GOAL-17.4)

```rust
const ANN_MIN_MEMORIES: usize = 100;   // below this, exact is fine
const EXACT_MAX_MEMORIES: usize = 2000; // hard cap for exact fallback

if n < ANN_MIN_MEMORIES {
    // Use exact O(n²) — fast enough for small n
    self.discover_exact(memories)
} else {
    match self.discover_ann(memories) {
        Ok(candidates) => candidates,
        Err(_) => {
            // ANN failed — sample down to 2000 and do exact
            let sampled = reservoir_sample(memories, EXACT_MAX_MEMORIES);
            self.discover_exact(&sampled)
        }
    }
}
```

## 5. API Changes

No public API changes (GUARD-2). Internal refactor only:

```rust
impl TopicDiscovery {
    // NEW builder method
    pub fn with_top_k(mut self, k: usize) -> Self {
        self.top_k = k;
        self
    }
    
    // discover() signature unchanged
    pub fn discover(&self, memories: &[(String, Vec<f32>)]) -> Vec<TopicCandidate> {
        // Now dispatches to discover_ann() or discover_exact()
    }
    
    // Internal methods
    fn discover_ann(&self, memories: &[(String, Vec<f32>)]) -> Result<Vec<TopicCandidate>, ...>
    fn discover_exact(&self, memories: &[(String, Vec<f32>)]) -> Vec<TopicCandidate>
}
```

## 6. Performance Budget

| Operation | n=16,366, dim=64 | Target |
|-----------|-----------------|--------|
| HNSW build | ~800ms | < 2s |
| Top-20 queries (all nodes) | ~1.5s | < 3s |
| Infomap (sparse, 320k edges) | ~2s | < 10s |
| **Total** | ~4.3s | **< 15s** |

vs. current: **infinite** (hangs, OOM killed)

## 7. Test Plan

1. **Existing tests pass unchanged** — `test_discover_basic_two_clusters` etc.
2. **New test: large synthetic** — 5000 memories with 5 known clusters, verify all found
3. **New test: scaling** — 16k random embeddings complete in < 15s
4. **New test: fallback** — verify exact computation kicks in for n < 100
5. **New test: degenerate** — 0, 1, 2 memories don't panic

## 8. Implementation Steps

1. Add `instant-distance` to `Cargo.toml`
2. Add `top_k` field to `TopicDiscovery` struct + builder method
3. Extract current loop into `discover_exact()`
4. Implement `discover_ann()` with HNSW
5. Update `discover()` to dispatch based on n
6. Raise default `edge_threshold` to 0.4
7. Configure Infomap with `num_trials(3)`
8. Add tests
9. Benchmark with 16k real memories
