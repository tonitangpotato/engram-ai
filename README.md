# 🧠 Engram

### Memory that forgets, strengthens, and dreams — like yours.

[![crates.io](https://img.shields.io/crates/v/engramai.svg)](https://crates.io/crates/engramai)
[![docs.rs](https://docs.rs/engramai/badge.svg)](https://docs.rs/engramai)
[![Tests](https://img.shields.io/badge/tests-309_passing-brightgreen)](https://github.com/tonitangpotato/engram-ai)
[![Lines of Rust](https://img.shields.io/badge/Rust-18%2C300_lines-orange)](https://github.com/tonitangpotato/engram-ai)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![Zero unsafe](https://img.shields.io/badge/unsafe-0-brightgreen)](https://github.com/tonitangpotato/engram-ai)

**Engram** is a memory system for AI agents grounded in cognitive neuroscience. Not a vector database with a wrapper — a faithful implementation of how biological memory actually works.

Every module maps to a real cognitive science model: [ACT-R](https://en.wikipedia.org/wiki/ACT-R) (Anderson 1993), [Ebbinghaus forgetting curves](https://en.wikipedia.org/wiki/Forgetting_curve) (1885), [Hebbian learning](https://en.wikipedia.org/wiki/Hebbian_theory) (Hebb 1949), [STDP](https://en.wikipedia.org/wiki/Spike-timing-dependent_plasticity) (Markram 1997), [synaptic homeostasis](https://en.wikipedia.org/wiki/Synaptic_homeostasis_hypothesis) (Tononi & Cirelli), and [dual-trace consolidation](https://en.wikipedia.org/wiki/Memory_consolidation) (McClelland 1995).

```
Your agent doesn't just search — it remembers.
Frequently used memories stay strong. Neglected ones fade.
Related memories reinforce each other. Sleep consolidates what matters.
Clusters of memories generate new insights — with full provenance.
```

---

## How Engram Compares

| Feature | Engram | Mem0 | Zep | Letta |
|---|:---:|:---:|:---:|:---:|
| **Activation decay** (memories fade over time) | ✅ ACT-R | ❌ | ❌ | — |
| **Associative strengthening** (co-recalled memories link) | ✅ Hebbian + STDP | ❌ | ❌ | ❌ |
| **Sleep consolidation** (periodic memory transfer) | ✅ Dual-trace | ❌ | ❌ | ❌ |
| **Automatic insight synthesis** (clusters → new knowledge) | ✅ 4-signal | ❌ | ❌ | ❌ |
| **Emotional valence tracking** | ✅ Per-domain | ❌ | ❌ | ❌ |
| **Hybrid search** (FTS + vector + activation) | ✅ 3-signal | ✅ 3-signal | ✅ | ✅ |
| **Multi-agent memory** | ✅ Namespaces | ✅ | ✅ | ✅ |
| **CJK tokenization** (Chinese/Japanese/Korean) | ✅ jieba-rs | — | — | — |
| **Zero external dependencies** | ✅ SQLite only | ❌ Postgres/Redis | ❌ Postgres | ❌ Postgres |
| **Language** | Rust | Python | Go | Python |
| **Embedding required?** | Optional | Required | Required | Required |

---

## Quick Start

```toml
# Cargo.toml
[dependencies]
engramai = "0.2"
```

```rust
use engramai::{Memory, MemoryType};

let mut mem = Memory::new("./agent.db", None)?;

// Store
mem.add(
    "potato prefers action over discussion",
    MemoryType::Relational,
    Some(0.7), None, None,
)?;

// Recall (FTS + vector + ACT-R activation fusion)
let results = mem.recall("what does potato prefer?", 5, None, None)?;
for r in results {
    println!("[{}] {}", r.confidence_label, r.record.content);
}

// Sleep — consolidate important memories, fade weak ones
mem.consolidate(1.0)?;
```

That's it. SQLite file, no Docker, no Redis, no Postgres.

---

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                     Agent / LLM                          │
└────────────────────────┬─────────────────────────────────┘
                         │
          ┌──────────────┼──────────────┐
          ▼              ▼              ▼
   ┌────────────┐ ┌───────────┐ ┌──────────────┐
   │   Memory   │ │ Emotional │ │   Session    │
   │   (core)   │ │    Bus    │ │ Working Mem  │
   └──┬─────┬───┘ └─────┬─────┘ └──────────────┘
      │     │           │
      ▼     ▼           ▼
┌──────────┐ ┌──────────────────────┐
│  Hybrid  │ │  Synthesis Engine    │
│  Search  │ │  cluster → gate →   │
│ FTS+Vec  │ │  insight → provenance│
│  +ACT-R  │ └──────────────────────┘
└──────────┘
      │
┌─────┴──────────────────────────────┐
│  ACT-R  │ Ebbinghaus │ Hebbian+STDP│
│  decay  │ forgetting │ associative  │
│  model  │ curves     │ learning     │
└─────────┴────────────┴─────────────┘
                  │
            ┌─────┴─────┐
            │  SQLite    │
            │ (WAL mode) │
            └───────────┘
```

---

## Cognitive Science Modules

### 🧮 ACT-R Activation (`models/actr.rs`)

Base-level activation from frequency + recency. Memories accessed more often and more recently have higher retrieval probability. Spreading activation from contextually related memories.

### 📉 Ebbinghaus Forgetting (`models/ebbinghaus.rs`)

Exponential decay curves with spaced repetition. Each successful recall resets the decay clock and extends the retention interval.

### 🔗 Hebbian Learning (`models/hebbian.rs`)

"Neurons that fire together wire together." Co-recalled memories form bidirectional associative links with strength that grows with repeated co-activation.

### ⏱️ STDP — Spike-Timing Dependent Plasticity

Temporal ordering matters: if memory A is consistently recalled *before* memory B, the A→B link strengthens while B→A weakens. Infers causal relationships from access patterns.

### 😴 Consolidation (`models/consolidation.rs`)

"Sleep" cycle transfers high-activation short-term memories to long-term storage. Weakly-activated memories decay further. Based on dual-trace hippocampus → neocortex transfer theory.

### 📊 Synaptic Homeostasis (Downscaling)

Global weight normalization prevents activation scores from inflating unboundedly. Based on Tononi & Cirelli's Synaptic Homeostasis Hypothesis — like the brain's nightly recalibration during sleep.

---

## Hybrid Search

Three signals fused — not just "vector search with extra steps":

```
Score = 0.15 × FTS5(BM25) + 0.60 × cosine(embedding) + 0.25 × ACT-R(activation)
```

- **FTS5**: Full-text search with BM25 ranking + jieba-rs CJK tokenization
- **Vector**: Cosine similarity (Nomic, Ollama, or any OpenAI-compatible endpoint)
- **ACT-R**: Base-level activation — biases toward memories that are *currently relevant*, not just semantically similar

Adaptive mode auto-adjusts weights based on query characteristics. Weights are fully configurable.

---

## Synthesis Engine (~4,100 lines)

Automatic insight generation from memory clusters — the most unique feature:

```
Memories → Cluster (4-signal) → Quality Gate → LLM Insight → Provenance → Store
```

1. **Clustering**: Groups memories using Hebbian weight + entity overlap + embedding similarity + temporal proximity
2. **Gate**: Quality check — minimum cluster size, type diversity, information density
3. **Insight**: Type-aware LLM prompts (factual patterns, episodic threads, causal chains)
4. **Provenance**: Full audit trail — which memories contributed, reversible via `undo_synthesis()`

```rust
let report = mem.synthesize()?;
println!(
    "Found {} clusters, synthesized {} insights",
    report.clusters_found,
    report.insights_created.len()
);
```

---

## Emotional Bus

Cognitive bus for agent self-awareness (~2,500 lines):

- **Emotional Accumulator** — tracks valence (positive/negative) per domain over time
- **Drive Alignment** — scores how well memories align with core drives (cross-language: Chinese SOUL + English content)
- **Behavior Feedback** — tracks tool success/failure rates for self-correction
- **Cross-Agent Subscriptions** — agents subscribe to namespaces, get notified of high-importance memories

---

## Advanced Examples

<details>
<summary><b>LLM Extraction</b> — raw text → structured facts</summary>

```rust
use engramai::{Memory, OllamaExtractor, AnthropicExtractor};

let mut mem = Memory::new("./agent.db", None)?;

// Local Ollama
mem.set_extractor(Box::new(OllamaExtractor::new("llama3.2:3b")));
// Or Anthropic Claude
// mem.set_extractor(Box::new(AnthropicExtractor::new("sk-ant-...", false)));

// "我昨天和小明一起吃了火锅" →
//   - "User ate hotpot yesterday with Xiaoming" (episodic, 0.5)
//   - "Xiaoming mentioned going to Shanghai next week" (factual, 0.7)
mem.add(
    "We decided to use PostgreSQL for the main DB and Redis for caching.",
    MemoryType::Factual, None, None, None,
)?;
```
</details>

<details>
<summary><b>Emotional Tracking</b> — per-domain sentiment</summary>

```rust
use engramai::bus::{EmotionalBus, EmotionalAccumulator, Drive, score_alignment_hybrid};

let bus = EmotionalBus::new("./workspace", &conn)?;

// Record emotional signals per domain
let acc = EmotionalAccumulator::new(&conn)?;
acc.record_emotion("coding", 0.8)?;   // positive: shipped feature
acc.record_emotion("coding", -0.3)?;  // negative: CI broke

let trends = bus.get_trends(&conn)?;
// → coding: net +0.5, trending positive

// Drive alignment (cross-language: Chinese drives + English content)
let drives = vec![Drive {
    name: "financial_freedom".into(),
    description: "帮 potato 实现财务自由".into(),
    keywords: vec!["revenue".into(), "profit".into(), "财务".into()],
}];
let score = score_alignment_hybrid("revenue increased 20%", &drives, None, None);
```
</details>

<details>
<summary><b>Multi-Agent Shared Memory</b></summary>

```rust
use engramai::{Memory, MemoryType, SubscriptionManager};

let mut mem = Memory::new("./shared.db", None)?;

// Store to namespace-isolated areas
mem.add_to_namespace(
    "Found critical security vuln in auth",
    MemoryType::Factual, Some(0.9), None, None,
    Some("coder"),
)?;

// CEO subscribes to all namespaces, importance ≥ 0.8
let subs = SubscriptionManager::new(mem.connection())?;
subs.subscribe("ceo", "*", 0.8)?;

// Recall from a specific namespace
let results = mem.recall_from_namespace("security issues", 5, None, None, Some("coder"))?;

let alerts = subs.check_notifications("ceo")?;
// → [Notification { content: "Found critical security...", namespace: "coder" }]
```
</details>

<details>
<summary><b>Agent Presets</b></summary>

```rust
use engramai::MemoryConfig;

let config = MemoryConfig::chatbot();            // Slow decay, high replay
let config = MemoryConfig::task_agent();          // Fast decay, low replay
let config = MemoryConfig::personal_assistant();  // Very slow core decay
let config = MemoryConfig::researcher();          // Minimal forgetting
```
</details>

---

## Memory Types

| Type | Use Case | Example |
|---|---|---|
| `Factual` | Facts, knowledge | "Rust 1.75 introduced async fn in traits" |
| `Episodic` | Events, experiences | "Deployed v2.0 at 3am, broke prod" |
| `Procedural` | How-to, processes | "To deploy: cargo build --release, scp, restart" |
| `Relational` | People, connections | "potato prefers Rust over Python" |
| `Emotional` | Feelings, reactions | "Frustrated by the third CI failure today" |
| `Opinion` | Preferences, views | "GraphQL is overengineered for most use cases" |
| `Causal` | Cause → effect | "Skipping tests → prod outage last Tuesday" |

---

## Performance

All core operations (add, recall, consolidate) are pure local SQLite — no network calls, no server roundtrips. The only operations that touch the network are optional embedding generation and LLM-based synthesis.

SQLite WAL mode — concurrent reads, single-writer, zero deployment dependencies.

---

## Why Not Just a Vector DB?

Vector databases give you `store → embed → retrieve`. That's a filing cabinet, not memory.

Real memory is *alive*:

| What biological memory does | Vector DB | Engram |
|---|---|---|
| Frequently used memories stay strong | ❌ All equal | ✅ ACT-R activation |
| Unused memories fade | ❌ Permanent | ✅ Ebbinghaus decay |
| Related memories reinforce each other | ❌ Independent | ✅ Hebbian links |
| Temporal ordering implies causality | ❌ No concept | ✅ STDP |
| Sleep consolidates what matters | ❌ No concept | ✅ Dual-trace transfer |
| Clusters generate new knowledge | ❌ No concept | ✅ Synthesis engine |
| Emotional context affects retrieval | ❌ No concept | ✅ Emotional bus |

The difference: vector search answers "what's similar?" — memory answers "what's *relevant right now*?"

---

## Project Structure

```
crates/engramai/src/
├── lib.rs              # Public API re-exports
├── memory.rs           # Core: add, recall, consolidate, forget
├── storage.rs          # SQLite backend, migrations, embeddings
├── hybrid_search.rs    # 3-signal search fusion (FTS5 + vector + ACT-R)
├── confidence.rs       # Two-dimensional confidence scoring
├── embeddings.rs       # Provider abstraction (Ollama, OpenAI-compat)
├── extractor.rs        # LLM fact extraction (Anthropic, Ollama)
├── entities.rs         # Rule-based entity extraction (Aho-Corasick)
├── anomaly.rs          # Sliding-window z-score anomaly detection
├── session_wm.rs       # Bounded working memory (Miller's Law: 7±2)
├── config.rs           # Agent presets (chatbot, researcher, etc.)
├── types.rs            # Core types
├── models/
│   ├── actr.rs         # ACT-R base-level activation
│   ├── ebbinghaus.rs   # Forgetting curves + spaced repetition
│   ├── hebbian.rs      # Associative link formation
│   └── consolidation.rs # Sleep-cycle memory transfer
├── synthesis/
│   ├── engine.rs       # cluster → gate → insight → provenance
│   ├── cluster.rs      # 4-signal memory clustering
│   ├── gate.rs         # Quality gate
│   ├── insight.rs      # LLM prompt construction
│   ├── provenance.rs   # Audit trail (reversible insights)
│   └── types.rs
└── bus/
    ├── mod.rs          # EmotionalBus core
    ├── mod_io.rs       # Drive, Identity, SoulUpdate types
    ├── alignment.rs    # Drive alignment (cross-language)
    ├── accumulator.rs  # Emotional valence per domain
    ├── feedback.rs     # Action success/failure tracking
    └── subscriptions.rs # Cross-agent notifications
```

---

## Design Philosophy

1. **Grounded in science, not marketing.** Every module maps to a published cognitive model with citations.
2. **Memory is not retrieval.** Retrieval is one operation. Memory includes forgetting, strengthening, consolidation, and synthesis.
3. **Provenance is non-negotiable.** Every synthesized insight records which memories contributed. Auditable and reversible.
4. **Zero deployment dependencies.** SQLite (bundled), pure Rust. Copy the binary and the `.db` file — done.
5. **Embeddings are optional.** The system works without any embedding provider (FTS5 + ACT-R only). Add embeddings for better semantic search — the cognitive models work independently.

---

## License

AGPL-3.0-or-later. See [LICENSE](LICENSE).

## Citation

```bibtex
@software{engramai,
  title = {Engram: Neuroscience-Grounded Memory for AI Agents},
  author = {Toni Tang},
  year = {2026},
  url = {https://github.com/tonitangpotato/engram-ai},
  note = {Rust. ACT-R, Hebbian learning, Ebbinghaus forgetting, STDP, cognitive synthesis.}
}
```
