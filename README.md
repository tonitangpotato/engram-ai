# Engram AI 🧠

**Neuroscience-grounded memory for AI agents — ACT-R activation, Hebbian learning, cognitive consolidation**

[![License](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE)
[![Python](https://img.shields.io/badge/python-3.9+-blue)](https://www.python.org/)
[![PyPI](https://img.shields.io/pypi/v/engramai)](https://pypi.org/project/engramai/)

> Give your AI agent a brain that actually remembers, associates, and forgets like a human.

### Available in 3 runtimes:

| Runtime | Package | Use When |
|---------|---------|----------|
| 🐍 **Python** | [`engramai` on PyPI](https://pypi.org/project/engramai/) | Python agents, MCP server, Claude Desktop |
| 🦀 **Rust** | [`engramai` on crates.io](https://crates.io/crates/engramai) — [GitHub](https://github.com/tonitangpotato/engram-ai-rust) | Performance-critical, embedded, zero-dependency |
| 📦 **TypeScript** | [`engram-ts/`](engram-ts/) | Node.js agents, OpenClaw |
| 🔌 **OpenClaw Plugin** | [`openclaw-plugin/`](openclaw-plugin/) | **Drop-in replacement** for OpenClaw's default context engine |

---

## What is Engram?

Most AI agents use a dumb FIFO window for context — keep the last N messages, throw away the rest. Engram replaces this with a **cognitive memory system** based on how human brains actually work:

- 🧠 **ACT-R Activation** — Memories strengthen with use, decay with time (not a fixed window)
- 🔗 **Hebbian Learning** — "Neurons that fire together, wire together" — related memories auto-associate
- 💤 **Consolidation** — Periodic forgetting + strengthening cycles (like sleep)
- 🎯 **Working Memory** — Miller's Law 7±2 chunks, automatic topic-change detection
- 🌍 **Cross-Language Search** — Recall memories across 50+ languages
- 💰 **Zero Cost** — Heuristic capture + prompt caching = $0 additional spend

## 🔌 OpenClaw Context Engine Plugin

**The fastest way to use Engram** — replace OpenClaw's default FIFO context with cognitive scoring in one config line:

```yaml
# openclaw.yaml
plugins:
  slots:
    contextEngine: engram
  entries:
    engram:
      autoCapture: true
```

Instead of keeping the last N messages, Engram scores every message by **ACT-R activation × Hebbian association × content importance**, then packs the most cognitively relevant ones into the token budget. Long-term memories persist across sessions automatically.

| | Default (Legacy) | Engram |
|--|--------|--------|
| Strategy | Keep last N messages (FIFO) | Score by cognitive relevance |
| Long-term memory | None | ACT-R + Hebbian |
| Cross-session | Compaction summaries (LLM cost) | Persistent recall (zero cost) |
| Topic awareness | None | Working memory + topic detection |
| Auto-capture | None | Preferences, facts, decisions, corrections |
| Cost | LLM summarization tokens | Zero (heuristic) |

→ **[Full plugin docs](openclaw-plugin/README.md)** | **[Source](openclaw-plugin/src/)**

---

## 🏆 Battle-Tested in Production

Real numbers from a live AI agent running 24/7:

| Metric | Value |
|--------|-------|
| **Memories stored** | 3,846 |
| **Recalls served** | 230,103 |
| **Hebbian links formed** | 12,510 (automatic) |
| **Consolidation layers** | 320 working → 224 core → 3,302 archive |
| **Database size** | 48 MB |
| **Recall latency** | ~90ms |
| **Additional cost** | **$0** (prompt caching absorbs overhead) |

---

## Quick Start

### Python

```bash
pip install engramai
```

```python
from engram import Memory

memory = Memory("./my-agent.db")

# Store
memory.add("User prefers detailed explanations", type="relational", importance=0.8)
memory.add("Project deadline: Feb 10", type="factual")

# Recall (ACT-R activation + Hebbian association + semantic similarity)
results = memory.recall("user preferences", limit=5)

# Consolidate (Ebbinghaus forgetting + strengthening)
memory.consolidate(days=1.0)
```

### TypeScript

```typescript
import { Memory } from 'neuromemory-ai';

const memory = new Memory('./my-agent.db');

memory.add("User prefers TypeScript over JavaScript", { type: "preference", importance: 0.8 });

const results = memory.recall("user language preferences", { limit: 5 });
```

### Rust

```rust
use engramai::Memory;

let mut memory = Memory::new("./my-agent.db", None)?;

memory.add("User prefers Rust for systems programming", "preference", 0.8, None)?;

let results = memory.recall("user preferences", 5, None, None)?;
```

### MCP Server (Claude Desktop, etc.)

```bash
export ENGRAM_DB_PATH=./my-agent.db
python3 -m engram.mcp_server
```

---

## 🆕 Engram v2: Multi-Agent Intelligence

Engram v2 adds powerful features for building **multi-agent systems** with shared memory, emotional feedback, and cross-agent intelligence:

### 🔐 Namespace Isolation & ACL

Separate memory spaces for different agents/domains with fine-grained access control:

```python
from engram import Memory
from engram.acl import Permission

memory = Memory("./shared.db")
memory.set_agent_id("trading_agent")

# Store in namespace
memory.add_to_namespace(
    "Oil prices spiked 15%",
    type="factual",
    importance=0.9,
    namespace="trading"
)

# ACL: Grant read permission to another agent
memory.grant("ceo_agent", "trading", Permission.READ)

# Subscribe to high-importance events
memory.subscribe("ceo_agent", "*", min_importance=0.8)

# Check for notifications
notifications = memory.check_notifications("ceo_agent")
# → [{memory_id: "...", namespace: "trading", content: "Oil prices...", importance: 0.9}]
```

**Use cases:**
- **CEO pattern** — Supervisor agent monitors specialist agents' discoveries
- **Team collaboration** — Agents share knowledge in topic-specific namespaces
- **Privacy isolation** — Sensitive memories stay in restricted namespaces

### 🎭 Emotional Bus — Memory ↔ Personality Feedback Loop

The Emotional Bus connects memory to agent workspace files (`SOUL.md`, `HEARTBEAT.md`), creating a **closed-loop** between what the agent experiences and how it evolves:

```python
from engram import Memory
from engram.bus import EmotionalBus

memory = Memory.with_emotional_bus(
    db_path="./agent.db",
    workspace_dir="./workspace"
)

# Store memory with emotional tracking
memory.add_with_emotion(
    "Debugging session took 3 hours with no progress",
    type="episodic",
    emotion=-0.8,  # Negative experience
    domain="debugging"
)

# Bus accumulates emotional trends
bus = memory.emotional_bus()
trends = bus.get_trends()
# → [EmotionalTrend(domain="debugging", valence=-0.75, count=5)]

# Suggest SOUL updates based on patterns
suggestions = bus.suggest_soul_updates()
# → [SoulUpdate(domain="debugging", action="add drive", 
#     content="Avoid lengthy debugging sessions without breaks")]

# Drive alignment boosts importance
# Memory matching SOUL drives gets automatic importance boost
bus.drives  # → [Drive(name="curiosity", description="...")]
```

**Workspace files:**

`SOUL.md` — Core drives/values:
```markdown
# Core Drives
curiosity: Always seek to understand new things
efficiency: Prefer action over endless discussion
```

`HEARTBEAT.md` — Periodic tasks:
```markdown
- [ ] Check email
- [x] Run consolidation
```

**How it works:**
1. **Memory → SOUL**: Negative emotional patterns trigger drive suggestions
2. **SOUL → Memory**: Memories aligned with drives get importance boost
3. **Behavior → HEARTBEAT**: Failed actions get deprioritization suggestions
4. **HEARTBEAT → Behavior**: Successful patterns get reinforced

**Use cases:**
- **Self-improving agents** — Personality evolves from experience
- **Emotional coherence** — Agent behavior aligns with "values"
- **Adaptive task scheduling** — HEARTBEAT learns what works

### 🌐 Cross-Namespace Intelligence

Hebbian links can span namespaces, enabling **cross-domain insights**:

```python
# Recall with cross-namespace associations
result = memory.recall_with_associations(
    "market volatility",
    namespace="*",  # Search all namespaces
    limit=5
)

# Returns both memories + cross-links
for link in result.cross_links:
    print(f"{link.source_ns}:{link.source_id} → {link.target_ns}:{link.target_id}")
    # trading:abc123 → geopolitics:def456 (strength: 0.85)
```

**Discovery pattern:**
```python
# Find connections between two domains
links = memory.discover_cross_links("trading", "geopolitics")
# → Reveals how trading events correlate with political events
```

---

## 🆕 v2.1.0: LLM Extraction & Hybrid Search

### 🤖 LLM-Powered Memory Extraction

Instead of storing raw conversation text, **extract key facts automatically**:

```python
from engram import Memory, AnthropicExtractor

memory = Memory("./agent.db")

# Option 1: Auto-configure from environment
# Just set ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN and it works automatically

# Option 2: Explicit setup
memory.set_extractor(AnthropicExtractor(api_key="sk-ant-..."))

# Now add() extracts facts automatically
memory.add("I had pizza yesterday and it was great, but my girlfriend didn't like it")
# Stores two separate facts:
# 1. "User likes pizza" (relational, importance 0.6)
# 2. "User's girlfriend doesn't like pizza" (relational, importance 0.6)
```

**Why it matters:**
- **Better recall**: Search for "user food preferences" finds "pizza" even if the word "preference" wasn't in the original text
- **Cleaner memory**: Facts are normalized, not duplicated across conversations
- **Cost-effective**: Uses Claude Haiku (~$0.0001 per memory) or local Ollama models (free)

### 🔧 Config Hierarchy

Engram now supports layered configuration with clear priority:

| Priority | Source | Use Case |
|----------|--------|----------|
| 1. Code | `memory.set_extractor(...)` | Agent harnesses (RustClaw, etc.) |
| 2. Env vars | `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_API_KEY` | Local dev, Docker |
| 3. Config file | `~/.config/engram/config.json` | User preferences |
| 4. No extractor | Falls back to raw text storage | Backward compatible |

```bash
# Initialize config interactively
python3 -m engram init
```

Config file (`~/.config/engram/config.json`):
```json
{
  "embedding": {
    "provider": "ollama",
    "model": "nomic-embed-text"
  },
  "extractor": {
    "provider": "anthropic",
    "model": "claude-haiku-4-5-20251001"
  }
}
```

> ⚠️ **Security**: Never store API keys in config files. Use environment variables.

### 🎯 Hybrid Search (FTS + Embedding + ACT-R)

`recall()` now uses **triple scoring** by default:

```python
results = memory.recall("user preferences", limit=5)
# Automatically combines:
#   15% FTS (exact term matching)
#   60% Embedding (semantic similarity)
#   25% ACT-R (recency/frequency/importance)
```

**Why three signals?**
- **FTS**: Catches exact matches (project names, technical terms)
- **Embedding**: Catches semantically similar concepts ("preference" ≈ "like")
- **ACT-R**: Prioritizes recently/frequently accessed memories

**Configurable weights** (Rust crate only, Python coming soon):
```rust
let mut config = MemoryConfig::default();
config.fts_weight = 0.30;        // 30% exact matching
config.embedding_weight = 0.50;   // 50% semantic
config.actr_weight = 0.20;        // 20% temporal
```

### 🌐 Cross-Language Drive Alignment (Rust only)

EmotionalBus drives written in one language now align with content in any other language:

```
SOUL.md: "帮potato实现财务自由，找到市场机会"  (Chinese)
Message: "trading profit market opportunity"   (English)

❌ Keyword matching: score = 0.0 (can't match across languages)
✅ Embedding alignment: score = 0.14 (semantic similarity via nomic-embed-text)
```

**How it works:**
- Drive descriptions are pre-embedded at startup (768-dim vectors via Ollama)
- At store time, content embeddings (already computed for recall) are reused — **zero additional cost**
- `score_alignment_hybrid()` = `max(keyword_score, embedding_score)` — best of both
- Same-language: keyword matching wins (precise, score=1.0)
- Cross-language: embedding matching wins (semantic, score=0.1-0.3)

### 🔄 Dynamic Token Refresh (Rust only)

The `TokenProvider` trait enables OAuth tokens that auto-refresh:

```rust
use engramai::{AnthropicExtractor, AnthropicExtractorConfig, TokenProvider};

struct MyOAuthProvider { /* ... */ }
impl TokenProvider for MyOAuthProvider {
    fn get_token(&self) -> Result<String, Box<dyn Error + Send + Sync>> {
        // Refresh token if expired, return valid token
    }
}

let extractor = AnthropicExtractor::with_token_provider(
    Box::new(MyOAuthProvider::new()),
    true,  // is_oauth
    AnthropicExtractorConfig::default(),
);
memory.set_extractor(Box::new(extractor));
// Token refreshes automatically before each extraction — no more 401 errors
```

### 🈶 CJK Tokenization (Rust only, Python coming soon)

Chinese/Japanese/Korean text now gets **intelligent word segmentation** via jieba:

```
❌ Before:
   "engram是认知记忆系统" → split into 8 characters
   Search "记忆系统" matches poorly

✅ After (with jieba):
   "engram是认知记忆系统" → ["engram", "是", "认知", "记忆系统"]
   Search "记忆系统" matches precisely
```

**Performance impact:** Negligible (~0.02ms per memory vs ~50ms for embedding generation)

---

## Core Concepts

### 🧠 ACT-R Activation

Every memory has an activation level that decays over time but strengthens with each access:

```
activation = base_level + spreading + importance_boost

base_level = ln(Σ tᵢ^(-d))    # frequency × recency decay
spreading  = Σ wⱼ × Sⱼᵢ       # context similarity
```

This means frequently-accessed, recent memories surface first — but old important memories can still be recalled if they're relevant to the current context.

### 🔗 Hebbian Learning

Memories that are recalled together form automatic associations:

```
ΔW = η × aᵢ × aⱼ    # co-activation strengthens links
```

Ask about "Docker" and "deployment" in the same conversation → they become linked. Next time you ask about deployment, Docker memories get boosted automatically.

### 💤 Consolidation

Like human sleep, periodic consolidation:
1. **Decays weak memories** (Ebbinghaus forgetting curve)
2. **Strengthens frequently-used ones** (move working → core → long-term)
3. **Prunes noise** (below-threshold memories removed)

### 🎯 Working Memory (TypeScript / OpenClaw Plugin)

Session-level state based on Miller's Law (7±2 chunks):
- Tracks what's "active" in the current conversation
- Detects topic changes → triggers full recall
- Continuous topic → reuses cached memories (70-80% fewer DB queries)

---

## 🧩 Memory Types

| Type | Use For | Example |
|------|---------|---------|
| `factual` | Facts and knowledge | "Project uses Python 3.12" |
| `episodic` | Events | "Shipped v2.0 on Jan 15" |
| `preference` | User preferences | "Prefers concise answers" |
| `procedural` | How-to knowledge | "Deploy: run tests first, then push" |
| `semantic` | Concepts and relationships | "Causal inference relates to Pearl" |
| `causal` | Cause-effect relationships | "Rate hikes → USD strengthens" |

---

## Architecture

```
                    ┌──────── STORE PATH ────────┐
                    │                             │
                    ▼                             │
               Input Text                        │
                    │                             │
          ┌────────▼────────┐                    │
          │ LLM Extraction  │ ← Claude Haiku     │
          │ (key facts)     │   (Rust only)      │
          └────────┬────────┘                    │
                   ▼                             │
          ┌────────────────┐                     │
          │ Embedding      │ ← nomic-embed-text  │
          │ (768-dim vec)  │   (local Ollama)    │
          └───┬────────────┘                     │
              │    ▼                             │
              │ ┌──────────────┐                 │
              │ │Drive Alignment│ ← pre-embedded  │
              │ │(importance↑)  │  SOUL drives    │
              │ └──────────────┘  (Rust only)    │
              ▼                                  │
          ┌────────────────┐                     │
          │ SQLite Storage │                     │
          │ text+FTS5+vec  │                     │
          └────────────────┘                     │
                                                 │
                    ┌──────── RECALL PATH ───────┘
                    ▼
               Query Text
                    │
          ┌────────▼────────┐
          │ Embedding       │ ← same model
          └────────┬────────┘
                   ▼
          ┌─────────────────────────┐
          │ Hybrid Search           │
          │  15% FTS (exact match)  │
          │  60% Embedding (cosine) │
          │  25% ACT-R (temporal)   │
          └──────────┬──────────────┘
                     ▼
          ┌─────────────────────────┐
          │ Hebbian Spreading       │ ← association boost
          │ Confidence Scoring      │ ← metacognition
          └──────────┬──────────────┘
                     ▼
               Ranked Results
```

---

## Configuration

### Embedding Providers

Engram works **without any embedding provider** (FTS5 keyword search). Add one for cross-language semantic search:

| Provider | Cost | Setup | Best For |
|----------|------|-------|----------|
| **None / FTS5** | Free | Zero config | Simple agents, testing |
| **Sentence Transformers** | Free | `pip install "engramai[sentence-transformers]"` | Privacy, offline |
| **Ollama** | Free | Ollama + embedding model | Already using Ollama |
| **OpenAI** | ~$0.0001/query | `OPENAI_API_KEY` | Highest quality |

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ENGRAM_EMBEDDING` | `auto` | Provider: `auto`, `sentence-transformers`, `ollama`, `openai`, `none` |
| `ENGRAM_DB_PATH` | `./engram.db` | Database path |
| `ENGRAM_ST_MODEL` | `paraphrase-multilingual-MiniLM-L12-v2` | Sentence Transformers model |
| `OPENAI_API_KEY` | — | Required for OpenAI embeddings |

---

## CLI

```bash
neuromem add "User prefers dark mode" --type preference --importance 0.8
neuromem recall "user preferences"
neuromem stats
neuromem consolidate
neuromem forget --threshold 0.01
neuromem hebbian "dark mode"
```

---

## 📚 Documentation

- **[OpenClaw Plugin Guide](openclaw-plugin/README.md)** — Drop-in context engine replacement
- **[Integration Guide](INTEGRATION-GUIDE.md)** — Level 3 auto-recall/store implementation
- **[Performance Analysis](PERFORMANCE.md)** — Production metrics and optimization
- **[Embedding Configuration](engram/EMBEDDING-CONFIG.md)** — Provider setup and tuning
- **[Vision](VISION.md)** — Where Engram is heading

---

## Development

```bash
git clone https://github.com/tonitangpotato/engram-ai.git
cd engram-ai

# Python
pip install -e ".[dev,all]"
pytest

# TypeScript
cd engram-ts && npm install && npm test

# OpenClaw Plugin
cd openclaw-plugin && npm install && npm test
```

---

## Credits

Built on research from:

- **ACT-R** (Adaptive Control of Thought-Rational) — Anderson, Carnegie Mellon
- **Hebbian Learning** — Donald Hebb, 1949
- **Memory Consolidation** — Walker & Stickgold (sleep research)
- **Forgetting Curve** — Hermann Ebbinghaus, 1885
- **Working Memory** — Baddeley & Hitch, 1974
- **Miller's Law** — George Miller, 1956 ("The Magical Number Seven")

---

## License

AGPL-3.0-or-later — see [LICENSE](LICENSE) for details. Commercial licensing available, see [COMMERCIAL-LICENSE.md](COMMERCIAL-LICENSE.md).

**[GitHub](https://github.com/tonitangpotato/engram-ai)** · **[PyPI](https://pypi.org/project/engramai/)** · **[crates.io](https://crates.io/crates/engramai)** · **[Issues](https://github.com/tonitangpotato/engram-ai/issues)**
