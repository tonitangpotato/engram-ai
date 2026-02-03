# Positioning: Engram vs Embedding-Based Memory

## Current Thesis

LLMs are already the semantic layer. Memory infrastructure should handle *dynamics* — when to surface, what to deprioritize, how to rank — using proven mathematical models rather than re-implementing semantic understanding with embeddings.

This differentiates engram from Mem0, Zep, and similar systems that are built on embedding + vector store.

## The Nuance: Not Anti-Vector, Anti-Naive

The positioning should not be "we don't use vectors" — it should be "we don't rely on naive embedding search."

**The problem with current embedding-based memory:**
- Embed text → store in vector DB → retrieve by cosine similarity
- No notion of decay, consolidation, or activation dynamics
- Treats all memories equally regardless of age, access frequency, or importance
- Semantically similar ≠ contextually relevant

**Why Hopfield networks matter here:**

The insight from "Hopfield Networks Is All You Need" (Ramsauer et al., 2020) is that transformer attention is essentially a modern Hopfield network retrieval step — query as probe, keys as stored patterns, softmax attention as energy minimization.

This means vector-based retrieval *can* be done in a cognitively grounded way:
- **Energy functions** instead of raw cosine similarity
- **Capacity theory** — principled limits on how many memories can coexist
- **Metastable states** — memories that are partially activated, not binary retrieved/not-retrieved
- **Biological basis** — associative recall grounded in how neural circuits actually work

Hopfield-style associative memory uses vectors, but the mechanism is fundamentally different from "throw it in Pinecone and do cosine search." It has mathematical guarantees about storage capacity, retrieval dynamics, and interference patterns.

## Refined Position

Engram's position should be:

> We don't re-implement semantic understanding with naive embeddings. Instead, we use mathematically rigorous models from cognitive science to handle memory dynamics. Where vector representations are useful (e.g., Hopfield-style associative recall), we use them within a cognitively grounded framework — not as a substitute for understanding how memory works.

This opens the door to:
1. **ACT-R** as the core activation/retrieval model (current implementation)
2. **Modern Hopfield networks** as an optional associative recall layer (future)
3. **Embedding-free operation** when paired with an LLM (current default)
4. **Embedding-enhanced operation** for standalone or high-volume scenarios

## Implications for Architecture

- Core recall stays embedding-free (ACT-R activation)
- Optional `AssociativeStore` interface for Hopfield-style retrieval
- When LLM is available, it handles semantics; engram handles dynamics
- When LLM is absent, Hopfield layer can fill the semantic gap with cognitive grounding
