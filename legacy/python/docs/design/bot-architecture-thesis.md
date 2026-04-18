# Bot Architecture Thesis: Memory-First Agent Framework

## The Three Pillars

### 1. Memory-First
Existing agent frameworks (LangChain, CrewAI, AutoGen) treat memory as an afterthought — a plugin, a retrieval layer, an optional add-on. Engram flips this: memory IS the agent's core.

- **Consciousness** = what's in working memory (context window)
- **Sleep** = consolidation cycles (scheduled)
- **Personality** = core memory (L2) + identity (L1)
- **Learning** = memory formation + consolidation
- **Forgetting** = activation decay + archival

The agent's behavior emerges from its memory dynamics, not from prompt engineering alone.

### 2. Portable Memory
Memory belongs to the agent, not the platform.

- Single `.db` file — `export()` and take it anywhere
- Switch LLM providers? Memory comes with you.
- Switch frameworks? Memory comes with you.
- Switch platforms? Memory comes with you.
- Agent death and rebirth? Import the `.db` and it remembers everything.

This is fundamentally different from platform-locked memory (Mem0 cloud, LangChain memory stores tied to specific vector DBs). An agent's memories are its identity — locking them to a platform is like erasing someone's past when they move to a new city.

### 3. Cognitively Grounded
Not naive embeddings. Not simple key-value stores. Real mathematical models from cognitive science:

- ACT-R activation for retrieval ranking
- Ebbinghaus forgetting curves for decay
- Memory Chain for dual-system consolidation
- Dopaminergic reward for reinforcement
- Synaptic homeostasis for downscaling
- Contradiction detection for self-correction

The math is simple. The insight is that these models, validated on human cognition over decades, map directly to the problems AI agents face with memory.

## Differentiation

| | LangChain/CrewAI | Mem0/Zep | Engram |
|---|---|---|---|
| Memory role | Plugin/afterthought | Dedicated layer | Core architecture |
| Retrieval | Embedding + cosine | Embedding + cosine | ACT-R + FTS5 + graph |
| Forgetting | None | None/manual | Ebbinghaus curves |
| Consolidation | None | None | Memory Chain (working→core) |
| Portability | Tied to vector DB | Cloud-locked | Single .db file |
| Dependencies | Vector DB + embedding model | API + cloud | Zero (pure Python/TS + SQLite) |
| Offline | No | No | Yes |

## Architecture Vision

```
┌─────────────────────────────────────┐
│           Agent Framework           │
│  ┌───────────────────────────────┐  │
│  │        LLM Interface          │  │
│  │   (any provider, swappable)   │  │
│  └──────────────┬────────────────┘  │
│                 │                    │
│  ┌──────────────▼────────────────┐  │
│  │         ENGRAM CORE           │  │
│  │  ┌─────────┐  ┌───────────┐  │  │
│  │  │ Working  │→→│   Core    │  │  │
│  │  │ Memory   │  │  Memory   │  │  │
│  │  │  (L3)    │  │   (L2)    │  │  │
│  │  └─────────┘  └───────────┘  │  │
│  │  ┌─────────┐  ┌───────────┐  │  │
│  │  │Identity │  │  Archive   │  │  │
│  │  │  (L1)   │  │   (L4)    │  │  │
│  │  └─────────┘  └───────────┘  │  │
│  │                               │  │
│  │  ACT-R │ Ebbinghaus │ Reward  │  │
│  │  Graph │ Consolidation │ FTS5 │  │
│  └──────────────────────────────┘  │
│                 │                    │
│           agent.db (portable)       │
└─────────────────────────────────────┘
```

## Key Insight

> An agent without portable memory is a prisoner of its platform.
> An agent without cognitive memory dynamics is a search engine with a personality prompt.
> Engram gives agents both: memories that work like minds, stored in files they own.
