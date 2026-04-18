# Paper Outline: Neuroscience-Grounded Memory for AI Agents

**Working Title:** "Beyond Vector Search: Cognitive Memory Dynamics for Language Model Agents"

**Target Venues:** NeurIPS, EMNLP, ACL, ICLR (2025-2026)

---

## Abstract (Draft)

Memory systems for AI agents typically reduce to embedding-based retrieval—storing text as vectors and retrieving by cosine similarity. This ignores decades of cognitive science research on how biological memory actually works. We present neuromemory-ai, a memory system that implements established models from cognitive psychology: ACT-R activation for retrieval, the Memory Chain Model for consolidation, and Ebbinghaus forgetting curves for decay. Our key insight is that large language models already provide semantic understanding; what they lack is principled *memory dynamics*—knowing when to surface information, what to deprioritize, and how knowledge should evolve over time. We introduce Hebbian learning for emergent memory associations without manual entity tagging. Experiments show [TODO: benchmarks] compared to Mem0, Zep, and shodh-memory on [TODO: agent tasks].

---

## 1. Introduction

### 1.1 The Problem
- AI agents (LangChain, AutoGPT, etc.) need persistent memory
- Current solutions: vector DBs + cosine similarity
- This treats memory as static search, not dynamic system

### 1.2 The Insight
- LLMs already handle semantics—they *understand* text
- What's missing: memory *dynamics*
  - When to surface (activation)
  - What to forget (decay)
  - How to consolidate (working → long-term)
  - How associations form (Hebbian learning)

### 1.3 Contributions
1. First implementation of ACT-R activation model for AI agent memory
2. Hebbian learning for emergent memory associations without NER
3. Memory Chain Model consolidation for dual-trace dynamics
4. Open-source library: neuromemory-ai (Python + TypeScript)
5. Benchmarks against existing solutions

---

## 2. Background & Related Work

### 2.1 Cognitive Science Models
- **ACT-R** (Anderson, 2007): Activation = base-level + spreading + importance
- **Memory Chain Model** (Murre & Chessa, 2011): Dual-trace consolidation
- **Ebbinghaus** (1885): Forgetting curves, spaced repetition
- **Hebbian Learning** (Hebb, 1949): "Neurons that fire together wire together"
- **Synaptic Homeostasis** (Tononi & Cirelli, 2006): Sleep and downscaling

### 2.2 AI Memory Systems
- **Mem0**: Vector search + manual memory management
- **Zep**: Vector + temporal filtering
- **shodh-memory**: Hebbian + TinyBERT NER, edge-first
- **LangChain Memory**: Simple buffer/summary patterns
- **HippoRAG** (Yu et al., 2024): Hippocampal-inspired retrieval

### 2.3 Gap in Literature
- Existing systems use engineering heuristics, not cognitive models
- No principled forgetting, consolidation, or activation
- Embedding redundancy when LLM is present

---

## 3. System Design

### 3.1 Architecture
```
┌─────────────────┐
│  LLM (external) │  ← Semantic understanding
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  neuromemory-ai │  ← Memory dynamics
│  ├── ACT-R      │
│  ├── Hebbian    │
│  ├── Forgetting │
│  └── Consolidate│
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  SQLite + FTS5  │  ← Storage
└─────────────────┘
```

### 3.2 ACT-R Activation Model
- Base-level activation: `B = ln(Σ t_k^(-d))`
- Spreading activation from context
- Importance modulation (amygdala analog)
- Mathematical derivation

### 3.3 Memory Chain Model
- Working memory (r₁): fast decay, recent traces
- Core memory (r₂): slow decay, consolidated knowledge
- Transfer dynamics: `dr₂/dt = α·r₁ - μ₂·r₂`
- Interleaved replay during consolidation

### 3.4 Hebbian Learning
- Track co-activation during recall
- Link formation after threshold (θ=3)
- Emergent associations without NER
- Why this replaces embedding-based entity linking

### 3.5 Ebbinghaus Forgetting
- Retrievability: `R(t) = e^(-t/S)`
- Stability growth with retrieval
- Memory-type specific decay rates

### 3.6 Additional Systems
- Synaptic downscaling (global normalization)
- Contradiction detection
- Two-dimensional confidence (reliability vs salience)
- Reward learning (dopaminergic feedback)

---

## 4. Implementation

### 4.1 Zero Dependencies
- Pure Python stdlib + SQLite
- No numpy, torch, or external APIs
- Design choice: maximize portability

### 4.2 Pluggable Storage
- SQLite (local, default)
- Supabase (cloud, planned)
- Cloudflare D1 (edge, planned)

### 4.3 Configuration Presets
- Chatbot: high replay, slow decay
- Task Agent: fast decay, procedural focus
- Personal Assistant: relationship memory, long-term
- Researcher: archive everything

---

## 5. Experiments

### 5.1 Evaluation Tasks
- **Multi-session agent continuity**: Does the agent remember across sessions?
- **Relevance vs recency**: Can it balance old important info vs new context?
- **Forgetting benefits**: Does forgetting improve retrieval quality?
- **Hebbian emergence**: Do meaningful associations form automatically?

### 5.2 Baselines
- Mem0 (vector-based)
- Zep (vector + temporal)
- shodh-memory (Hebbian + NER)
- Raw LLM context (no memory system)

### 5.3 Metrics
- Retrieval precision/recall
- User preference (human eval)
- Computational cost
- Memory growth over time

### 5.4 Results
[TODO: Run experiments]

---

## 6. Discussion

### 6.1 When to Use Cognitive Models
- LLM-native agents (neuromemory-ai)
- Edge/offline (shodh-memory with NER)
- Simple applications (raw vector search)

### 6.2 Limitations
- FTS5 keyword search less flexible than embeddings
- Requires LLM for semantic understanding
- Parameter tuning needed for different agents

### 6.3 Future Work
- Adaptive parameter tuning based on recall success
- Multi-agent shared memory
- Cloud sync with conflict resolution
- Integration with specific agent frameworks

---

## 7. Conclusion

Memory for AI agents should not be reduced to vector similarity search. By implementing established cognitive science models—ACT-R activation, Memory Chain consolidation, Ebbinghaus forgetting, and Hebbian learning—we create memory systems that behave more like biological memory: strengthening with use, fading without it, and forming emergent associations. neuromemory-ai demonstrates that these models can be implemented efficiently (zero dependencies, ~500 lines of core code) while providing principled alternatives to engineering heuristics.

---

## Appendices

### A. Mathematical Derivations
- Full ACT-R activation formula
- Memory Chain differential equations
- Forgetting curve with stability

### B. Implementation Details
- SQLite schema
- Hebbian link formation algorithm
- Configuration parameters

### C. Comparison Tables
- Feature comparison with existing systems
- Performance benchmarks

---

## References

- Anderson, J.R. (2007). How Can the Human Mind Occur in the Physical Universe?
- Murre, J.M.J. & Chessa, A.G. (2011). One hundred years of forgetting.
- Ebbinghaus, H. (1885). Über das Gedächtnis.
- Hebb, D.O. (1949). The Organization of Behavior.
- Tononi, G. & Cirelli, C. (2006). Sleep function and synaptic homeostasis.
- Yu, B. et al. (2024). HippoRAG: Neurobiologically Inspired Long-Term Memory for LLMs.

---

*Outline created: 2026-02-03*
*Status: Draft outline, needs experiments*
