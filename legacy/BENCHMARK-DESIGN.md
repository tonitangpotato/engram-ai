# Engram Benchmark Suite — Design Doc

> Testing what matters for cognitive memory, not just retrieval accuracy.

**Date**: 2026-03-22
**Status**: Draft
**Author**: potato + Clawd

---

## Why Existing Benchmarks Don't Fit

| Benchmark | What it tests | Why it's insufficient for Engram |
|-----------|--------------|----------------------------------|
| **LongMemEval** | Recall from long conversations (temporal, knowledge updates) | Tests RAG retrieval in single sessions. No activation decay, no consolidation, no forgetting. |
| **LoCoMo** | Long-context conversational memory | Measures context window utilization, not cognitive memory dynamics |
| **ConvoMem** | Conversational memory accuracy | Static evaluation — store facts, query facts. No lifecycle (strengthen/weaken/forget) |

**Core gap**: These benchmarks treat memory as a **database** (store → retrieve → check correctness). Engram treats memory as a **living system** (store → access → strengthen → decay → forget → associate → consolidate). We need benchmarks that test the *dynamics*, not just the endpoint.

---

## Engram Benchmark Dimensions

### Dimension 1: Activation & Decay (ACT-R)

**Hypothesis**: Frequently accessed memories should have higher activation and be recalled preferentially.

#### Test 1.1: Frequency Boost
```
Setup: Store 100 memories with equal importance
Action: Access memories #1-10 five times each, leave #11-100 untouched
Query: Ambiguous query that could match any of the 100
Metric: What % of top-10 results are from the accessed set?
Expected: >80% (vs 10% random baseline)
```

#### Test 1.2: Recency Decay
```
Setup: Store 50 memories at t=0, 50 more at t=7days
Action: No access to any
Query: Query matching both groups equally
Metric: Ratio of recent vs old memories in top-10
Expected: Recent memories ranked higher (recency decay effect)
```

#### Test 1.3: Recency vs Frequency Trade-off
```
Setup: Store memory A at t=0, access 20 times. Store memory B at t=6days, access 1 time.
Query: Query matching both A and B equally
Metric: Which ranks higher?
Expected: Depends on decay parameter. Test that tuning actr_decay shifts the balance predictably.
```

### Dimension 2: Consolidation Quality

**Hypothesis**: Consolidation should preserve important memories and forget noise, improving recall precision over time.

#### Test 2.1: Importance Survival
```
Setup: Store 500 memories — 100 high importance (0.8-1.0), 400 low importance (0.1-0.3)
Action: Run consolidate() 10 times (simulating 10 days)
Metric: Survival rate by importance bucket
Expected: >90% high-importance survive, <50% low-importance survive
```

#### Test 2.2: Precision After Forgetting
```
Setup: Store 200 relevant memories + 800 noise memories (random text)
Action: Access the 200 relevant ones, then consolidate 5 times
Query: Queries targeting the relevant memories
Metric: Precision@10 before vs after consolidation
Expected: Precision improves (noise forgotten, signal preserved)
```

#### Test 2.3: Consolidation Idempotency
```
Setup: Store 100 memories, consolidate 100 times
Metric: No memory with importance > 0.5 is lost
Expected: Consolidation converges, doesn't over-prune
```

### Dimension 3: Hebbian Association

**Hypothesis**: Co-activated memories should form links and boost each other's recall.

#### Test 3.1: Association Formation
```
Setup: Store memories A="Paris is the capital of France" and B="The Eiffel Tower is in Paris"
Action: Recall A, then recall B, repeat 5 times (co-activation)
Metric: hebbian_links(A) includes B? Strength > threshold?
Expected: Link formed with strength proportional to co-activation count
```

#### Test 3.2: Associative Recall Boost
```
Setup: Store 100 memories. Co-activate A and B 10 times. Do not co-activate A and C.
Query: Query matching A
Metric: Does B rank higher than C in recall results (assuming B and C are equally relevant by content)?
Expected: B ranks higher due to Hebbian boost
```

#### Test 3.3: Association Decay
```
Setup: Co-activate A and B 10 times. Wait (simulate time with consolidation).
Metric: Does Hebbian link strength decay? At what rate?
Expected: Decay follows configured hebbian_decay parameter
```

### Dimension 4: Cross-Language Recall

**Hypothesis**: With embeddings, storing in one language and querying in another should work.

#### Test 4.1: Chinese → English
```
Setup: Store "Polymarket的CLOB服务器在伦敦eu-west-2"
Query: "Where is the Polymarket CLOB server?"
Metric: Is the Chinese memory recalled?
Compare: FTS5-only (expected: fail) vs embedding (expected: success)
```

#### Test 4.2: Mixed Language Memory
```
Setup: Store 50 English memories + 50 Chinese memories about the same topics
Query: 25 English queries + 25 Chinese queries
Metric: Cross-language recall rate
Expected: >70% with embeddings, <20% without
```

### Dimension 5: Contradiction Handling

**Hypothesis**: When new information contradicts old, the system should prefer newer/more authoritative information.

#### Test 5.1: Simple Contradiction
```
Setup: At t=0, store "The API endpoint is api.v1.example.com"
       At t=1, store "The API endpoint changed to api.v2.example.com"
Query: "What is the API endpoint?"
Metric: Which memory ranks higher?
Expected: Newer memory ranks higher (recency) but both returned
```

#### Test 5.2: Contradiction with Importance
```
Setup: At t=0, store "X is true" with importance 0.9
       At t=1, store "X is false" with importance 0.5
Query: About X
Metric: Which wins — recency or importance?
Expected: Configurable behavior (some agents want authority, some want recency)
```

### Dimension 6: Multi-Agent Isolation

**Hypothesis**: Namespaced memories should not leak between agents.

#### Test 6.1: Namespace Isolation
```
Setup: Agent A stores "My API key is sk-xxx" in namespace "agent_a"
       Agent B queries "API key" in namespace "agent_b"
Metric: Agent B gets zero results
Expected: 100% isolation
```

#### Test 6.2: Shared Namespace
```
Setup: Agent A stores memory in namespace "shared"
       Agent B reads from namespace "shared"
Metric: Agent B can recall Agent A's memories
Expected: Full access within shared namespace
```

### Dimension 7: Temporal Reasoning

**Hypothesis**: The system should support time-based queries efficiently.

#### Test 7.1: Time Range Query
```
Setup: Store 100 memories per day for 30 days
Query: "What happened last week?"
Metric: Are results correctly filtered to the last 7 days?
Expected: >90% of results from correct time range
```

#### Test 7.2: Temporal Ordering
```
Setup: Store events A (t=1), B (t=2), C (t=3) about the same topic
Query: "What happened with [topic]?"
Metric: Are results returned in chronological order?
Expected: Correct ordering
```

### Dimension 8: Scale & Performance

#### Test 8.1: Recall Latency vs DB Size
```
Setup: Insert N memories (N = 100, 1K, 10K, 100K, 1M)
Query: Standard recall query
Metric: p50/p99 latency at each scale
Expected: <50ms for 10K, <200ms for 100K with FTS5. Embedding recall may be slower.
```

#### Test 8.2: Consolidation Time
```
Setup: DB with N memories
Action: consolidate()
Metric: Wall time
Expected: Linear or sub-linear in N
```

#### Test 8.3: Concurrent Access
```
Setup: DB with 10K memories
Action: 4 threads doing simultaneous recall + 1 thread doing add
Metric: No corruption, no deadlocks, acceptable latency
Expected: WAL mode handles this
```

---

## Comparison with Traditional Agent Memory

### What Traditional Systems Do

| System | Approach | Strengths | Weaknesses |
|--------|----------|-----------|------------|
| **Mem0** | LLM-extracted facts + vector DB | Good entity extraction, structured | Expensive (LLM per message), no forgetting |
| **Zep** | Session summaries + vector search | Good for conversation continuity | No cognitive model, no cross-session learning |
| **LangChain Memory** | Buffer/Summary/Entity memory types | Easy integration | Simplistic, no persistence, no decay |
| **MemGPT/Letta** | Agentic memory management | Agent controls own memory | High LLM cost, complex |
| **ChromaDB/Pinecone** | Pure vector store | Fast similarity search | No cognitive layer, just storage |

### Where Engram is Different

| Engram Feature | Traditional Equivalent | Engram's Advantage |
|---|---|---|
| ACT-R activation decay | None (all memories equal) | Naturally surfaces relevant memories without re-ranking |
| Hebbian association | Manual tagging/linking | Automatic association through usage patterns |
| Ebbinghaus forgetting | Manual deletion or TTL | Quality improves over time (noise decays) |
| Memory layers (working→core→longterm) | Flat storage | Models human memory hierarchy |
| Zero-cost capture (heuristic) | LLM per message ($$$) | No API cost for storage decisions |
| SQLite file = protocol | API server required | Zero infrastructure, portable |

### Where Engram Falls Short

| Gap | What's Missing | Impact | Priority |
|-----|---------------|--------|----------|
| **No entity/relation schema** | Mem0 stores structured entities + relations; engram stores flat text | Can't answer "who does X know?" without full-text scan | High |
| **Capture quality depends on caller** | Engram stores what it's told; quality of what gets stored is the bot's LLM responsibility, not engram's. Needs good integration APIs so bots can easily store structured, high-quality memories | Poor APIs → poor memories regardless of engram's cognitive model | High |
| **Passive contradiction handling** | `contradicts` field exists but not auto-detected | Stale/wrong info persists indefinitely | High |
| **Embedding gap across languages** | Only Python has Ollama, Rust/TS FTS5 only | Cross-language recall broken in Rust/TS | High (for unified goal) |
| **Weak temporal indexing** | No indexed time-range queries | "What happened last month" requires full scan | High |
| **Shallow Hebbian** | Single-hop only (A→B), no multi-hop (A→B→C) | Limited associative reasoning | Medium |
| **Emotional saliency is decorative** | emotional type is a label, doesn't affect activation | Surprising/scary events aren't remembered more strongly | Medium |
| **No sleep replay** | consolidate() is pure math decay, not replay | Missing a key biological consolidation mechanism | Medium |
| **No multi-modal** | Text only, no images/audio | Can't remember what things looked like | Low (for now) |

### Open Problems (entire field, not Engram-specific)

| Gap | Description | State of the Art |
|-----|-------------|-----------------|
| **Abstraction/generalization** | No system auto-merges "Paris is capital of France" + "Berlin is capital of Germany" → "European capitals" | Unsolved — even Mem0/MemGPT store facts, not concepts |
| **Causal chains** | Can't trace "X caused Y caused Z" | Graph DBs can store but not auto-discover causal links |
| **Concept formation** | Raw memories → abstract knowledge | Requires reasoning, not just storage |

---

## Benchmark Implementation Plan

### Phase 1: Core Tests (dimensions 1-3)
- [ ] Python test harness using pytest
- [ ] Synthetic memory generator (controllable importance, timestamps, content similarity)
- [ ] ACT-R activation tests (1.1, 1.2, 1.3)
- [ ] Consolidation quality tests (2.1, 2.2, 2.3)
- [ ] Hebbian association tests (3.1, 3.2, 3.3)
- [ ] Baseline: compare Engram vs flat vector DB (ChromaDB) on same queries

### Phase 2: Advanced Tests (dimensions 4-7)
- [ ] Cross-language test suite (requires embedding setup)
- [ ] Contradiction detection + resolution tests
- [ ] Multi-agent isolation tests
- [ ] Temporal reasoning tests

### Phase 3: Scale & Cross-Language
- [ ] Performance benchmarks at scale (10K → 1M memories)
- [ ] Run same benchmark on Python, Rust, TS implementations
- [ ] Compare results — if implementation X fails where Y succeeds, that's a bug

### Phase 4: Comparison Paper
- [ ] Run LongMemEval/LoCoMo/ConvoMem on Engram (for reference comparison)
- [ ] Run Engram benchmark on Mem0/Zep/LangChain (if APIs allow)
- [ ] Publish results + benchmark suite as open-source

---

## Success Criteria

The benchmark should prove or disprove:

1. **"Forgetting improves recall"** — precision@10 should increase after consolidation
2. **"Usage matters"** — frequently accessed memories should rank higher than equally relevant but unused ones
3. **"Association is automatic"** — co-activated memories should form retrievable links without manual tagging
4. **"One DB, any language"** — same benchmark results whether running Python, Rust, or TS
5. **"Zero-cost capture competes with LLM-based"** — Engram's heuristic capture vs Mem0's LLM extraction on standard memory tasks

---

*A good benchmark doesn't just measure — it reveals what to build next.*
