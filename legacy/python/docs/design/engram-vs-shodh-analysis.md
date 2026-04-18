# Engram vs Shodh-Memory: Design Analysis

> Discussion notes from 2026-02-02 late night session. Use for future documentation and paper.

## The Core Question

**Why does engram exist when shodh-memory already does similar things?**

## Architectural Philosophy

### shodh-memory: Self-Contained Memory System
```
┌─────────────────────────────────┐
│  shodh-memory (~17MB binary)    │
│  ├── TinyBERT NER (entities)    │
│  ├── TinyBERT Embedding (search)│
│  ├── Hebbian learning           │
│  ├── Activation decay           │
│  └── 3-tier memory overflow     │
└─────────────────────────────────┘
         ↓
   Can run standalone
   (edge devices, air-gapped, no LLM)
```

### engram: LLM-Native Memory Layer
```
┌─────────────────────────────────┐
│  Your LLM (external)            │
│  ├── Semantic understanding     │
│  └── Entity extraction (opt.)   │
└─────────────────────────────────┘
         ↓
┌─────────────────────────────────┐
│  engram (~50KB Python)          │
│  ├── ACT-R activation scoring   │
│  ├── Memory Chain consolidation │
│  ├── Ebbinghaus forgetting      │
│  ├── Hebbian learning           │
│  └── FTS5 keyword search        │
└─────────────────────────────────┘
```

**Key insight**: engram assumes you have an LLM. shodh doesn't.

## Why No NER in engram?

### shodh's Approach: TinyBERT NER
- Automatically extracts PERSON, ORG, LOCATION
- Creates knowledge graph connections
- Works without any LLM

### engram's Approach: LLM + Hebbian
- LLM already understands entities better than TinyBERT
- User/LLM can specify entities explicitly: `entities=[("potato", "likes")]`
- Hebbian learning creates emergent connections from usage patterns
- No extra model dependency

### Why This Makes Sense

| Aspect | TinyBERT NER | LLM extraction |
|--------|--------------|----------------|
| Understanding | Fixed categories (PERSON/ORG/LOC) | Arbitrary relationships |
| Context | Weak | Strong |
| "Rust" | Can't identify as programming language | Knows it's a language |
| Extra dependency | Yes (~67MB model) | No (already have LLM) |

**Conclusion**: For LLM agents, bundling NER is redundant. The LLM is already better at entity extraction.

## Why Hebbian Learning?

### The Problem
Without NER, how do memories form connections automatically?

### The Solution: Learn from Usage
```python
# Memories recalled together form connections
Query 1: "机器学习框架" → [PyTorch, TensorFlow, 神经网络]
Query 2: "深度学习工具" → [PyTorch, TensorFlow, 神经网络]  
Query 3: "Python ML"    → [PyTorch, TensorFlow, 神经网络]
                              ↓
# After 3 co-activations: automatic links formed
PyTorch ↔ TensorFlow ↔ 神经网络
```

### Neuroscience Basis
- **Hebbian theory**: "Neurons that fire together wire together" (Hebb, 1949)
- **Long-term potentiation (LTP)**: Repeated co-activation strengthens synaptic connections
- This is more "biologically authentic" than NER

### Hebbian vs NER: Different Purposes

| | NER | Hebbian |
|---|---|---|
| When | Write time (add memory) | Read time (recall) |
| Based on | Content analysis | Usage patterns |
| Creates | Semantic connections | Behavioral connections |
| Example | "Elon Musk" in two texts → linked | Two memories always recalled together → linked |

**They're complementary, not competing.** But Hebbian alone can bootstrap a useful graph without NER.

## Mathematical Rigor: engram's Differentiator

### shodh-memory's Math
```
Activation decay: A(t) = A₀ · e^(-λt)
```
Simple exponential decay. Only considers time since last access.

### engram's Math (ACT-R)
```
A = ln(Σ t_k^(-0.5)) + C + I
    ↑                  ↑   ↑
    Base-level         Context  Importance
    (all access times) (spreading activation)
```

**Key difference**: ACT-R considers EVERY access time, not just the most recent.

### Why This Matters

Same memory accessed 5 times vs 1 time, same recency:
- **shodh**: Same activation (only looks at most recent access)
- **engram**: 5-access memory has higher activation (power law of practice)

This matches human cognition: frequently used knowledge is more accessible, even if last accessed at the same time.

### Memory Consolidation

**shodh**: Capacity-based overflow (Working → Session → Long-term when full)

**engram**: Strength-based differential equations
```
dr₁/dt = -μ₁ · r₁           (working memory decays)
dr₂/dt = α · r₁ - μ₂ · r₂   (core memory grows from working)
```

This is the Memory Chain Model (Murre & Chessa, 2011) — mathematically models how memories consolidate over time, not just when buffers overflow.

## Target Users

### shodh-memory
- Edge devices (Raspberry Pi, IoT)
- Air-gapped/offline systems
- Cost-sensitive (avoid LLM API calls)
- "Just works" developers

### engram
- LLM agent developers
- Researchers (publishable math)
- Developers who want to understand WHY
- Minimal dependency requirements

## The Tagline

> **engram**: Memory dynamics for LLM agents — because your LLM already understands, it just needs to remember.

## Paper Contributions

1. **Positioning**: Memory systems should handle dynamics, not duplicate semantic understanding
2. **Hebbian for AI memory**: Emergent structure without NER dependency
3. **ACT-R in practice**: Real implementation with benchmarks
4. **Two-dimensional confidence**: Separating reliability from salience
5. **Comparison**: Rigorous math (engram) vs engineering heuristics (shodh/Mem0/Zep)

## Future Directions

1. **Benchmark**: Compare engram vs shodh on same workloads
2. **Adaptive parameters**: Self-tuning based on recall hit rate
3. **Spreading activation**: Full implementation through Hebbian + entity graphs
4. **Supabase backend**: For serverless (SaltyHall integration)

---

*Document created: 2026-02-02*
*For use in: README, docs, research paper*
