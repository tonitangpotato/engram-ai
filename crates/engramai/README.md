# Engram — Neuroscience-Grounded Memory for AI Agents

[![crates.io](https://img.shields.io/crates/v/engramai.svg)](https://crates.io/crates/engramai)
[![docs.rs](https://docs.rs/engramai/badge.svg)](https://docs.rs/engramai)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)

**engramai** is a memory system for AI agents grounded in cognitive neuroscience — not just a vector database with a wrapper. It models how biological memory actually works: activation decay, associative strengthening, consolidation during "sleep", anomaly detection, emotional valence tracking, and cross-agent knowledge synthesis.

**16,700 lines of Rust · 309 tests · Zero unsafe**

## Why Not Just Use a Vector DB?

Vector databases give you `store → embed → retrieve`. That's a filing cabinet, not memory.

Real memory is *alive*:
- **Memories you use often stay strong** — ACT-R activation modeling
- **Memories you don't use fade** — Ebbinghaus exponential forgetting
- **Related memories strengthen each other** — Hebbian associative learning
- **Temporal co-occurrence implies causality** — STDP (spike-timing dependent plasticity)
- **Sleep consolidates what matters** — dual-trace hippocampus → neocortex transfer
- **Clusters of related memories generate new insights** — synthesis engine with provenance tracking
- **Emotional patterns accumulate per domain** — not just what happened, but how it *felt*

The result: an agent that genuinely *remembers* — not one that performs semantic search and calls it memory.

## Architecture

```
                          ┌─────────────────────────────────┐
                          │         Agent / LLM             │
                          └──────────┬──────────────────────┘
                                     │
                    ┌────────────────┼────────────────┐
                    ▼                ▼                ▼
             ┌───────────┐   ┌────────────┐   ┌───────────────┐
             │  Memory    │   │ Emotional  │   │   Session     │
             │  (core)    │   │   Bus      │   │ Working Mem   │
             └─────┬──┬──┘   └─────┬──────┘   └───────────────┘
                   │  │            │
          ┌────────┘  └────┐      │
          ▼                ▼      ▼
   ┌─────────────┐  ┌──────────────────┐
   │ Hybrid      │  │ Synthesis Engine  │
   │ Search      │  │ (cluster→gate→   │
   │ FTS+Vec+ACT │  │  insight→prove)  │
   └──────┬──────┘  └──────────────────┘
          │
   ┌──────┴──────────────────────────┐
   ▼             ▼            ▼      ▼
┌───────┐ ┌──────────┐ ┌────────┐ ┌──────────┐
│ACT-R  │ │Ebbinghaus│ │Hebbian │ │Embeddings│
│decay  │ │forgetting│ │links   │ │(Nomic/   │
│model  │ │curves    │ │+ STDP  │ │ Ollama)  │
└───────┘ └──────────┘ └────────┘ └──────────┘
                    │
                    ▼
              ┌──────────┐
              │  SQLite   │
              │ (WAL mode)│
              └──────────┘
```

## Cognitive Science Modules

### Core Memory Models (`models/`)

| Module | Inspiration | What It Does |
|--------|------------|--------------|
| **ACT-R Activation** | Anderson's ACT-R | Base-level activation from frequency + recency. Memories accessed more often and more recently have higher retrieval probability. Spreading activation from contextually related memories. |
| **Ebbinghaus Forgetting** | Ebbinghaus 1885 | Exponential decay curves with spaced repetition. Each successful recall resets the decay clock and extends the retention interval. Configurable decay rates per agent type. |
| **Hebbian Learning** | Hebb's Rule | "Neurons that fire together wire together." Co-recalled memories form bidirectional associative links with strength that grows with repeated co-activation. |
| **STDP** | Spike-Timing Dependent Plasticity | Temporal ordering matters: if memory A is consistently recalled *before* memory B, the A→B link strengthens while B→A weakens. Infers causal relationships from access patterns. |
| **Consolidation** | Dual-trace theory | "Sleep" cycle transfers high-activation short-term memories to long-term storage. Weakly-activated memories decay further. Configurable replay count and consolidation threshold. |

### Hybrid Search (`hybrid_search.rs`)

Not just vector similarity. Three signals fused with configurable weights:

```
Final Score = w_fts × FTS5_score + w_vec × cosine_sim + w_actr × activation
              (15%)                  (60%)                (25%)
```

- **FTS5**: Full-text search with BM25 ranking + jieba-rs CJK tokenization
- **Vector**: Cosine similarity on embeddings (Nomic, Ollama, or any OpenAI-compatible endpoint)
- **ACT-R**: Base-level activation from access history — biases toward memories that are *currently relevant*, not just semantically similar

Adaptive mode auto-adjusts weights based on query characteristics.

### Confidence Scoring (`confidence.rs`)

Two-dimensional confidence assessment — because "how relevant is this?" and "how reliable is this?" are different questions:

- **Retrieval Salience**: How strongly does this memory match the query? (search score + activation + recency)
- **Content Reliability**: How trustworthy is this memory? (access count + corroboration by other memories + consistency)
- **Labels**: `high` / `medium` / `low` / `uncertain` — human-readable confidence for LLM consumption

### Synthesis Engine (`synthesis/`)

The most sophisticated module — **3,500+ lines** implementing automatic insight generation from memory clusters:

```
Memories → Cluster Discovery → Gate Check → LLM Insight → Provenance → Store
              (4-signal)       (quality)    (templated)    (auditable)
```

1. **Clustering** (`cluster.rs`): Groups related memories using 4 signals — Hebbian link weight, entity overlap (Jaccard), embedding cosine similarity, temporal proximity. Not k-means — uses actual cognitive association strength.

2. **Gate** (`gate.rs`): Quality gate prevents junk synthesis. Checks minimum cluster size, diversity of memory types, information density, temporal spread. Only clusters that pass the gate proceed.

3. **Insight Generation** (`insight.rs`): Constructs type-aware LLM prompts (factual patterns, episodic threads, causal chains) from cluster members. Parses and validates the LLM output.

4. **Provenance** (`provenance.rs`): Every synthesized insight records its full provenance chain — which memories contributed, what cluster they came from, which gate criteria were met. Insights are auditable and reversible (`UndoSynthesis`).

### Emotional Bus (`bus/`)

A cognitive bus system for agent self-awareness — **2,500+ lines** across 6 sub-modules:

- **Emotional Accumulator** (`accumulator.rs`): Tracks emotional valence (positive/negative) per domain over time. Detects when a domain is trending persistently negative → suggests SOUL.md updates.
- **Drive Alignment** (`alignment.rs`): Scores how well new memories align with the agent's core drives (from SOUL.md). Embedding-based scoring handles cross-language naturally (Chinese SOUL + English content).
- **Behavior Feedback** (`feedback.rs`): Tracks action success/failure rates. Which tools work? Which consistently fail? Surfaces behavioral patterns for self-correction.
- **Subscriptions** (`subscriptions.rs`): Cross-agent intelligence — agents subscribe to namespaces and get notified of high-importance memories. Enables CEO pattern (supervisor monitors all specialists).
- **Drive Embeddings** (`alignment.rs`): Pre-computed embeddings for SOUL drives at startup. Cross-language alignment threshold at 0.3 cosine similarity.

### Anomaly Detection (`anomaly.rs`)

Sliding-window z-score anomaly detection on any metric:
- Maintains per-metric baselines with configurable window sizes
- Flags values that deviate significantly from recent history
- Used internally for monitoring memory system health, available for agent-level anomaly detection

### Session Working Memory (`session_wm.rs`)

Mimics human working memory — a small, fast, per-session cache:
- Bounded capacity (configurable, default ~7 items — Miller's Law)
- Automatic eviction of least-recently-used items
- Session-scoped: cleared on session end, not persisted
- Avoids redundant DB queries within a conversation turn

### Entity Extraction (`entities.rs`)

Rule-based entity recognition using Aho-Corasick + regex:
- Extracts Projects, People, Technologies, Concepts from text
- No LLM needed — fast pattern matching
- Entities used for Jaccard similarity in clustering and for cross-referencing

### LLM Extraction (`extractor.rs`)

Optional structured fact extraction before storage:
- **Anthropic Claude**: Via API (Haiku recommended for cost)
- **Ollama**: Local models (llama3.2:3b, etc.)
- Raw text → multiple typed memory records with importance scores

```rust
// Input: "我昨天和小明一起吃了火锅，他说下周要去上海出差。"
// Output:
//   - "User ate hotpot yesterday with Xiaoming" (episodic, 0.5)
//   - "Xiaoming will travel to Shanghai next week" (factual, 0.7)
```

## Quick Start

```rust
use engramai::{Memory, MemoryType};

let mut mem = Memory::new("./agent.db", None)?;

// Store a memory
mem.add(
    "potato prefers action over discussion",
    MemoryType::Relational,
    Some(0.7),
    None,
    None,
)?;

// Recall with hybrid search (FTS + vector + ACT-R)
let results = mem.recall("what does potato prefer?", 5, None, None)?;
for r in results {
    println!("[{}] {}", r.confidence_label, r.record.content);
}

// Run "sleep" cycle — consolidate important memories
mem.consolidate(1.0)?;
```

### With LLM Extraction

```rust
use engramai::{Memory, OllamaExtractor, AnthropicExtractor};

let mut mem = Memory::new("./agent.db", None)?;

// Use local Ollama for extraction
mem.set_extractor(Box::new(OllamaExtractor::new("llama3.2:3b")));

// Or Anthropic Claude
// mem.set_extractor(Box::new(AnthropicExtractor::new("sk-ant-...", false)));

// Raw text → automatically extracted as structured facts
mem.add(
    "We decided to use PostgreSQL for the main DB and Redis for caching. \
     The team agreed this is non-negotiable.",
    MemoryType::Factual,
    None, None, None,
)?;
```

### With Emotional Bus

```rust
use engramai::bus::{EmotionalBus, Drive, Identity};

let bus = EmotionalBus::new(&conn);

// Track emotional valence per domain
bus.record_emotion("coding", 0.8, "Successfully shipped feature")?;
bus.record_emotion("coding", -0.3, "CI broke again")?;

// Get trends
let trends = bus.get_trends()?;
// → coding: net +0.5, trending positive

// Drive alignment
let drives = vec![Drive { text: "帮 potato 实现财务自由".into(), weight: 1.0 }];
let identity = Identity { drives, ..Default::default() };
let score = bus.score_alignment(&identity, "revenue increased 20%")?;
```

### With Synthesis Engine

```rust
use engramai::synthesis::types::{SynthesisSettings, SynthesisEngine};

let settings = SynthesisSettings::default();

// Discover clusters, gate-check, generate insights
let report = mem.synthesize(&settings)?;

for insight in &report.insights {
    println!("Insight: {}", insight.content);
    println!("From {} memories, confidence: {:.2}", 
        insight.provenance.source_count, insight.importance);
}

// Undo a synthesis if the insight was wrong
mem.undo_synthesis(insight_id)?;
```

## Memory Types

| Type | Use Case | Example |
|------|----------|---------|
| `Factual` | Facts, knowledge | "Rust 1.75 introduced async fn in traits" |
| `Episodic` | Events, experiences | "Deployed v2.0 at 3am, broke prod" |
| `Procedural` | How-to, processes | "To deploy: cargo build --release, scp, systemctl restart" |
| `Relational` | People, connections | "potato prefers Rust over Python for systems work" |
| `Emotional` | Feelings, reactions | "Frustrated by the third CI failure today" |
| `Opinion` | Preferences, views | "GraphQL is overengineered for most use cases" |
| `Causal` | Cause → effect | "Skipping tests → prod outage last Tuesday" |

## Configuration

### Agent Presets

```rust
use engramai::MemoryConfig;

let config = MemoryConfig::chatbot();            // Slow decay, high replay
let config = MemoryConfig::task_agent();          // Fast decay, low replay  
let config = MemoryConfig::personal_assistant();  // Very slow core decay
let config = MemoryConfig::researcher();          // Minimal forgetting
```

### Embedding Configuration

Embeddings are optional. Without them, search uses FTS5 + ACT-R only.

```rust
use engramai::EmbeddingConfig;

// Local Ollama (recommended for privacy)
let config = EmbeddingConfig {
    provider: "ollama".into(),
    model: "nomic-embed-text".into(),
    endpoint: "http://localhost:11434".into(),
    ..Default::default()
};

// Or any OpenAI-compatible endpoint
let config = EmbeddingConfig {
    provider: "openai-compatible".into(),
    model: "text-embedding-3-small".into(),
    endpoint: "https://api.openai.com/v1".into(),
    api_key: Some("sk-...".into()),
    ..Default::default()
};
```

### Search Weight Tuning

```rust
use engramai::HybridSearchOpts;

let opts = HybridSearchOpts {
    fts_weight: 0.15,       // Full-text search contribution
    embedding_weight: 0.60,  // Vector similarity contribution
    activation_weight: 0.25, // ACT-R activation contribution
    ..Default::default()
};
```

### CJK Support

Chinese/Japanese/Korean tokenization is built-in via `jieba-rs`. No configuration needed — FTS5 queries with CJK content are automatically segmented.

## Multi-Agent Architecture

### Shared Memory with Namespaces

Multiple agents can share a single database with namespace isolation:

```rust
// Agent 1: coder
let mut coder_mem = Memory::new("./shared.db", Some("coder"))?;

// Agent 2: researcher  
let mut research_mem = Memory::new("./shared.db", Some("researcher"))?;

// CEO agent subscribes to all namespaces
let subs = SubscriptionManager::new(&conn);
subs.subscribe("ceo", "*", 0.8)?; // All namespaces, importance ≥ 0.8

// When coder stores something important, CEO gets notified
coder_mem.add("Found critical security vulnerability in auth module",
    MemoryType::Factual, Some(0.9), None, None)?;

let notifications = subs.poll("ceo")?;
// → [Notification { content: "Found critical security...", namespace: "coder" }]
```

### Session Working Memory for Conversations

```rust
use engramai::SessionWorkingMemory;

let wm = SessionWorkingMemory::new(7); // Miller's Law: 7±2 items

// During a conversation turn, cache frequently accessed memories
wm.put("current_topic", recall_result);

// Fast retrieval without DB hit
if let Some(cached) = wm.get("current_topic") {
    // Use cached result
}
// Automatically evicts LRU items when capacity exceeded
```

## Performance

| Operation | Time | Notes |
|-----------|------|-------|
| `add()` (no extraction) | ~1ms | SQLite insert + index update |
| `add()` (with embedding) | ~50ms | + embedding API call |
| `add()` (with LLM extraction) | ~500ms | + LLM API call |
| `recall()` (FTS only) | ~2ms | BM25 ranking |
| `recall()` (hybrid) | ~60ms | FTS + vector + ACT-R fusion |
| `consolidate()` | ~10ms | Per memory batch |
| `synthesize()` | ~1-5s | Depends on cluster count + LLM |

SQLite with WAL mode — concurrent reads, single-writer, zero deployment dependencies.

## Installation

```toml
[dependencies]
engramai = "0.2"
```

## Project Structure

```
crates/engramai/src/
├── memory.rs          # Core Memory struct — add, recall, consolidate, forget
├── storage.rs         # SQLite backend — schema, migrations, embedding storage
├── hybrid_search.rs   # 3-signal search fusion (FTS5 + vector + ACT-R)
├── confidence.rs      # Two-dimensional confidence scoring
├── embeddings.rs      # Embedding provider abstraction (Ollama, OpenAI-compat)
├── extractor.rs       # LLM fact extraction (Anthropic, Ollama)
├── entities.rs        # Rule-based entity extraction (Aho-Corasick)
├── anomaly.rs         # Sliding-window z-score anomaly detection
├── session_wm.rs      # Bounded working memory (per-session cache)
├── config.rs          # MemoryConfig presets
├── types.rs           # Core types (MemoryRecord, MemoryType, HebbianLink, etc.)
├── models/
│   ├── actr.rs        # ACT-R base-level activation
│   ├── ebbinghaus.rs  # Forgetting curves with spaced repetition
│   ├── hebbian.rs     # Associative link formation
│   └── consolidation.rs  # Sleep-cycle memory transfer
├── synthesis/
│   ├── engine.rs      # Orchestration: cluster → gate → insight → provenance
│   ├── cluster.rs     # 4-signal memory clustering
│   ├── gate.rs        # Quality gate for synthesis candidates
│   ├── insight.rs     # LLM prompt construction + output parsing
│   ├── provenance.rs  # Audit trail for synthesized insights
│   └── types.rs       # Synthesis type definitions
└── bus/
    ├── mod.rs         # EmotionalBus core (SOUL integration)
    ├── mod_io.rs      # Drive/Identity types, I/O
    ├── alignment.rs   # Drive alignment scoring (embedding-based, cross-language)
    ├── accumulator.rs # Emotional valence tracking per domain
    ├── feedback.rs    # Action success/failure rate tracking
    └── subscriptions.rs  # Cross-agent notification system
```

## Design Philosophy

1. **Grounded in science, not marketing.** Every module maps to a real cognitive science model with citations. ACT-R (Anderson 1993), Ebbinghaus (1885), Hebbian learning (Hebb 1949), STDP (Markram 1997), dual-trace consolidation (McClelland 1995).

2. **Memory is not retrieval.** Vector search answers "what's similar?" — memory answers "what's *relevant right now*?". The difference is activation, context, emotional state, and temporal dynamics.

3. **Provenance is non-negotiable.** Every synthesized insight records exactly which memories contributed. Insights can be audited and undone. No black-box "the AI said so."

4. **Zero deployment dependencies.** SQLite (bundled), pure Rust. No external database, no Docker, no Redis. Copy the binary and the .db file — that's your entire deployment.

5. **Embeddings are optional.** The system works without any embedding provider (FTS5 + ACT-R only). Add embeddings for better semantic search, but the cognitive models work independently.

## License

AGPL-3.0-or-later. See [LICENSE](LICENSE) for details.

## Citation

If you use engramai in research:

```bibtex
@software{engramai,
  title = {Engram: Neuroscience-Grounded Memory for AI Agents},
  author = {Toni Tang},
  year = {2026},
  url = {https://github.com/tonioyeme/engram},
  note = {Rust implementation. ACT-R, Hebbian learning, Ebbinghaus forgetting, cognitive synthesis.}
}
```
