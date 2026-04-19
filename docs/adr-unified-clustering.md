# Unified Clustering Engine

> Architecture decision record: merge two independent clustering systems into one.

## Date

2026-04-18

## Context

engram had **two independent clustering implementations** that did fundamentally the same thing — group related memories into clusters — but with different algorithms and different signal inputs:

| Aspect | `compiler/discovery.rs` | `synthesis/cluster.rs` |
|--------|------------------------|----------------------|
| Algorithm | Infomap (upgraded from agglomerative) | Union-Find (fixed threshold) |
| Signals | Embedding cosine only | 4 signals: Hebbian + Entity + Embedding + Temporal |
| Purpose | Topic page discovery | Sleep-cycle insight generation |
| Output | `TopicCandidate` | `MemoryCluster` |

The duplication meant:
- Two codepaths to maintain for the same core operation
- Inconsistent clustering quality (Infomap >> Union-Find)
- synthesis missed out on Infomap's information-theoretic community detection
- No way to share improvements between the two systems

## Decision

**Merge into a single `clustering.rs` module** with a strategy pattern for edge weights.

### Architecture

```
┌─────────────────────────────────┐
│       InfomapClusterer<S>       │  ← src/clustering.rs (NEW)
│  (builds k-NN graph, runs      │
│   Infomap, returns clusters)    │
└──────────┬──────────────────────┘
           │ uses S::compute_weight()
    ┌──────┴──────┐
    ▼             ▼
EmbeddingOnly   MultiSignal
(cosine sim)    (hebbian + entity +
                 embedding + temporal)
```

### Key types

- **`EdgeWeightStrategy`** trait — single method: `compute_weight(a, b) -> f64`
- **`EmbeddingOnly`** — cosine similarity only (used by compiler)
- **`MultiSignal`** — 4-signal weighted combination (used by synthesis)
- **`InfomapClusterer<S>`** — generic clusterer parameterized by strategy
- **`ClusterNode`** — carries all signals (embedding, hebbian links, entities, timestamp)
- **`Cluster`** — output: member indices + centroid + cohesion score

### What changed

| File | Change |
|------|--------|
| `src/clustering.rs` | **NEW** — unified Infomap engine + strategy trait + 2 built-in strategies |
| `src/lib.rs` | Added `pub mod clustering` |
| `src/compiler/discovery.rs` | Thin adapter: builds `ClusterNode`s from `(id, embedding)` pairs, delegates to `InfomapClusterer<EmbeddingOnly>`, converts output to `TopicCandidate` |
| `src/synthesis/cluster.rs` | Adapter: loads signals from Storage, builds `ClusterNode`s, delegates to `InfomapClusterer<MultiSignal>`, converts to `MemoryCluster` |
| `Cargo.toml` | `infomap-rs` now non-optional (was behind `kc` feature flag) |

### What was removed

- Union-Find clustering from `synthesis/cluster.rs` (replaced by Infomap)
- `connected_components()` function and its tests
- Redundant Infomap code from `compiler/discovery.rs` (now in shared module)

### Public API preserved

All external-facing types and functions remain unchanged:
- `TopicDiscovery::discover()` — same signature
- `discover_clusters()` — same signature
- `MemoryCluster`, `TopicCandidate` — same structs
- `compute_pairwise_signals()`, `compute_composite_score()` — still available

## Consequences

- **synthesis gets Infomap for free** — no more fixed-threshold Union-Find
- **Future strategies** (e.g., adding sentiment similarity) only need a new `impl EdgeWeightStrategy`
- **compiler could optionally upgrade** to MultiSignal if Hebbian/entity data becomes available
- `infomap-rs` is now a required dependency (previously optional behind `kc` feature)

## Test Results

- **696 tests pass** (575 lib + 121 integration/doc)
- **0 warnings**
- New tests added in `clustering.rs` for the unified engine
