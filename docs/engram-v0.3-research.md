# Agent Memory Systems — Comparative Research (2026-04-23)

> Context for engram v0.3 design. Read this, then DESIGN-v0.3.md.
>
> Systems surveyed: **Graphiti, mem0, A-MEM, Letta (MemGPT), LightRAG**, with engram as baseline.
> Goal: extract what each does well, where it fails, and what engram should absorb vs reject.

---

## 0. Why These Five

| System | Why it matters | What we want from it |
|---|---|---|
| **Graphiti** (Zep) | Strongest OSS KG memory, production-grade, 25k⭐ | Entity/edge model, temporal bi-validity, hybrid search |
| **mem0** | Simplest LLM-driven memory, 35k⭐, arXiv paper w/ LOCOMO results | ADD/UPDATE/DELETE/NONE decision prompt, API shape |
| **A-MEM** (Princeton) | Only system with **memory evolution** — new memories retro-update old ones | Retro-evolution mechanism |
| **Letta / MemGPT** (UCB) | Only system where the **agent itself** manages memory via tools | Block-based hierarchy, self-editing via function calls |
| **LightRAG** (HKU) | SOTA on RAG benchmarks; **dual-level retrieval** (local entity + global topic) | Dual-level query strategy |

Everything else in the landscape (LangChain ConversationKGMemory, Zep v1, RAG-Anything, …) is either superseded by one of these or not agent-memory-specific.

---

## 1. Side-by-side at a glance

| Capability | Graphiti | mem0 | A-MEM | Letta | LightRAG | engram (today) |
|---|---|---|---|---|---|---|
| **Storage backend** | Neo4j | Qdrant/Pinecone + optional graph | Vector DB | Postgres + files | NetworkX + vector | **SQLite (embedded)** |
| **Entity identity** | ✅ UUID nodes | ❌ facts only | ❌ notes only | ❌ free-text blocks | ✅ LLM-extracted | ⚠️ extracted but not persisted |
| **Typed edges** | ✅ schema-constrained | ❌ | ❌ | ❌ | ✅ LLM-labeled | ⚠️ Triple exists, not used as edges |
| **Temporal bi-validity** | ✅ valid_at / invalid_at | ❌ | ❌ | ❌ | ❌ | ⚠️ superseded_by, no valid range |
| **Provenance (episode → fact)** | ✅ | ⚠️ source msg id | ❌ | ❌ | ⚠️ chunk id | ⚠️ source string field |
| **Activation / decay** | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ **ACT-R + dual-trace (r1, r2)** |
| **Affect / valence** | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ 11-dim incl. valence/arousal |
| **Interoceptive regulation** | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ unique |
| **Consolidation (strength → LTM)** | ❌ | ❌ | ⚠️ periodic rewrite | ⚠️ archival flush | ❌ | ✅ Murre & Chessa ODE |
| **Knowledge synthesis (topic abstraction)** | ⚠️ community detect | ❌ | ✅ note evolution | ❌ | ✅ high-level keys | ✅ **Knowledge Compiler** |
| **Retro-update of old memories** | ❌ | ⚠️ UPDATE action | ✅ **signature feature** | ⚠️ via agent tool | ❌ | ⚠️ via Compiler/consolidation |
| **Hybrid retrieval (vec + bm25 + graph)** | ✅ | ⚠️ vec only | ⚠️ vec only | ⚠️ vec + recall | ✅ **dual-level** | ✅ present but under-used |
| **Agent-driven memory management** | ❌ | ❌ | ❌ | ✅ **signature feature** | ❌ | ❌ |
| **LLM calls per write** | 5–10 | **2** (extract + decide) | 1 + 1 per evolved neighbor | varies (agent-driven) | 1–2 | 1 (extractor) |
| **Metacognition / confidence** | ❌ | ❌ | ❌ | ⚠️ heuristic | ❌ | ✅ confidence + metacognition |
| **LOCOMO benchmark (public)** | ~62% (reported) | 68.5% (paper) | not reported | not reported | not reported | not run yet |

**Empty cells are honest** — these are opportunities, not oversights.

---

## 2. Detailed per-system notes

### 2.1 Graphiti — structured correctness

**Architecture**: episode in → extract entities → extract edges → per-entity dedup (LLM) → per-edge resolution (LLM: duplicate/invalidate/new) → commit to Neo4j.

**What they got right:**
- Entity nodes are first-class with UUID, attributes, summary, `episode_mentioned_in`
- Edges carry `valid_at` + `invalid_at` — the right abstraction for facts that change over time
- **Invalidate, never delete** — preserves history, enables "what did I believe on date X" queries
- Episode is the atomic provenance unit — every edge points back to the episode that asserted it
- Hybrid search = vector + BM25 + graph traversal (RRF fused)

**What they got wrong / skipped:**
1. **Cost**: 5–10 LLM calls per episode. Production users report this is the #1 pain point.
2. **No importance signal** — every fact is equal until invalidated. Yesterday's lunch and your wedding anniversary are the same.
3. **No decay** — the graph only grows. "Is this still true?" ≠ "is this still relevant?" They only answer the first.
4. **Schema is rigid** — edge types must be predeclared in an `edge_type_map`. Real conversation spills outside any predeclared taxonomy.
5. **Neo4j dependency** — you can't embed Graphiti in a binary. Heavy for an agent library.
6. **No affect / context dimensions** — purely propositional.
7. **Binary entity decisions** — either merged or not; no "probabilistic identity".

**Takeaway for engram**: adopt entity + edge + bi-temporal + invalidation. Reject the brute-force LLM approach and the monolithic Neo4j assumption.

---

### 2.2 mem0 — minimal LLM pipeline

**Architecture**: `add(messages)` →
1. LLM call 1 — extract facts from messages (structured list of short strings)
2. Vector-search existing memories for each fact
3. LLM call 2 — decide per fact: **ADD / UPDATE / DELETE / NONE** (given old + new)
4. Apply actions to vector store

That's it. No graph (the graph version is a thin LightRAG-style bolt-on that only adds 2% on LOCOMO).

**What they got right:**
- **The 4-action decision prompt is excellent** — it's the cleanest way to reconcile new info with old info using a single LLM call. This is the core insight we should steal.
- Flat vector store keeps latency low (91% lower p95 than full-context baseline)
- Dead-simple API: `add`, `search`, `get_all`, `update`, `delete`

**What they got wrong / skipped:**
- No identity — "Melanie" in memory #1 and "Melanie" in memory #500 are unrelated strings
- No temporal validity — UPDATE overwrites, you lose "what did I believe then"
- No decay, no affect, no structure, no consolidation
- The paper's LOCOMO score is the *LLM-judged* number; rigorous re-eval shows it's closer to mid-50s. Good, not SOTA.

**Takeaway for engram**: **steal the ADD/UPDATE/DELETE/NONE decision prompt** — use it as engram's edge-resolution LLM call when cheap path is uncertain. This is the single most valuable thing mem0 contributes.

---

### 2.3 A-MEM — retro-evolution

**Architecture** (Zettelkasten-inspired):
1. `add(note)` → LLM generates note with tags, context, keywords, links
2. Vector-search k nearest neighbors
3. LLM call — "given this new note and these neighbors, should the neighbors' context/tags be updated?"
4. **Mutate old notes** based on the LLM's judgment

**What they got right (and nobody else does):**
- **Backward influence** — new knowledge changes how old knowledge is described. This is genuinely biological: learning a new concept retro-labels old episodes.
- Links between notes are bidirectional and LLM-generated (not just vector similarity)

**What they got wrong / skipped:**
- Evolution runs every write → O(k) extra LLM calls per add; expensive
- No entity identity — notes, not graph nodes
- No temporal model
- Evaluation is weak (their own benchmarks, not LOCOMO)

**Takeaway for engram**: **retro-evolution is the gap in our consolidation story**. Current engram consolidation only *strengthens/decays* old memories — it doesn't *rewrite* their metadata in light of new info. This belongs in the offline audit / consolidation cycle, not on every write.

---

### 2.4 Letta (MemGPT) — agent-as-memory-manager

**Architecture**:
- Memory split into **blocks**: `core` (always in context), `archival` (searchable store), `recall` (message history)
- Agent has **tools**: `core_memory_append`, `core_memory_replace`, `archival_memory_insert`, `archival_memory_search`, `conversation_search`
- The LLM **chooses** when to move things between blocks via tool calls

**What they got right:**
- Puts memory curation in the agent's hands — the agent can decide "this is important enough to pin in core"
- Clean separation: hot (core) / warm (recall) / cold (archival)
- Self-healing: agent can rewrite/replace stale core blocks

**What they got wrong / skipped:**
- Memory quality depends entirely on agent's tool-use discipline — brittle with smaller models
- No structural model underneath — core blocks are just markdown strings
- No affect, no decay, no graph
- LOCOMO results are uninspired — the agent often forgets to write to memory

**Takeaway for engram**: block-based hot/warm/cold tiers map onto engram's `working_strength (r1) / core_strength (r2) / archived`. We already have this implicitly via consolidation; **expose it as a formal tier and let the agent query/pin across tiers**. Don't copy the "agent drives everything" philosophy — it's too fragile.

---

### 2.5 LightRAG — dual-level retrieval

**Architecture**:
1. Chunk text → extract entities + relations → vector index + KG
2. Query comes in → LLM classifies query intent:
   - **Low-level / specific** ("what is X's role?") → retrieve entity-centric subgraph
   - **High-level / abstract** ("what are the key themes?") → retrieve topic-level keys
3. Hybrid retrieval fuses both levels

**What they got right:**
- **Query classification before retrieval** — matches retrieval strategy to intent
- Dual indexes (entity-level + topic-level) let the system answer both narrow and broad questions
- Cheaper than GraphRAG (Microsoft) at comparable quality

**What they got wrong / skipped:**
- Built for RAG (static corpus), not for agent memory (streaming, evolving)
- No temporal model, no entity resolution over time
- No affect, no consolidation

**Takeaway for engram**: we already have a `query_classifier` module. **Formalize dual-level retrieval**: specific queries hit the entity/edge graph; abstract queries hit Knowledge Compiler topics. Both levels fuse via RRF.

---

## 3. The pattern across all five

Every system solves **one** of these dimensions:

| Dimension | Champion |
|---|---|
| Structured correctness | Graphiti |
| Ergonomic simplicity | mem0 |
| Backward influence | A-MEM |
| Self-management | Letta |
| Query-aware retrieval | LightRAG |
| **Affect + decay + regulation** | **engram (only one)** |

**No system does all six.** That's the gap engram fills — not by inventing a new paradigm, but by being the first to **integrate all six**.

---

## 4. What engram must steal (and what to reject)

### Steal (and adapt):
1. **Graphiti's bi-temporal edges** — `valid_at` / `invalid_at` on relations; never delete, only invalidate
2. **Graphiti's entity/edge schema with provenance** — every edge points to the episode that created it
3. **mem0's ADD/UPDATE/DELETE/NONE prompt** — use this as engram's LLM tie-breaker, not its default path
4. **A-MEM's retro-evolution** — run during consolidation cycle (offline), not every write
5. **Letta's tiered blocks** — expose hot/warm/cold as an API, map onto our existing working/core/archived
6. **LightRAG's dual-level query routing** — specific → graph, abstract → Knowledge Compiler topics

### Reject:
1. **Graphiti's 5–10 LLM calls per episode** — brute force; engram's multi-signal fusion can do better
2. **mem0's flat vector store** — no structure means no reasoning
3. **A-MEM's evolve-on-every-write** — too expensive; batch in consolidation
4. **Letta's agent-drives-everything** — too fragile; use tools as *opt-in* for the agent, not the only path
5. **LightRAG's static-corpus assumption** — we're streaming, temporal, alive

### Keep (non-negotiable — engram's moat):
- ACT-R activation + dual-trace consolidation (Murre & Chessa)
- 11-dim affective metadata (valence, arousal, domain, …)
- Interoceptive regulation + somatic markers
- Metacognition + confidence
- Knowledge Compiler (topic synthesis)
- Hebbian links
- SQLite-embedded deployment

---

## 5. The synthesis that becomes v0.3

> **engram v0.3 = (Graphiti's graph layer) × (mem0's reconciliation prompt) × (A-MEM's retro-evolution) × (Letta's tier API) × (LightRAG's dual-level retrieval) × (engram's cognitive/affective core)**

The graph is *new scaffolding* on top of existing memory records — not a replacement. ACT-R runs on top. Affect runs on top. Consolidation operates on both layers. Knowledge Compiler sits above both.

See `DESIGN-v0.3.md` for the concrete architecture.
