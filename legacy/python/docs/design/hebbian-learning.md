# Hebbian Learning in Engram

## What is Hebbian Learning?

> **"Neurons that fire together, wire together."**  
> — Donald Hebb, *The Organization of Behavior* (1949)

Hebbian learning is a fundamental principle in neuroscience that describes how neural connections strengthen through correlated activity. When two neurons are repeatedly activated together, the synapse between them becomes stronger, making future co-activation more likely.

In the context of AI agent memory, Hebbian learning means:
- When two memories are **recalled together** (co-activated), they form an association
- After repeated co-activation (above a threshold), a **link** is automatically created
- These links enable **spreading activation** — retrieving one memory makes related memories more accessible
- Links have **strength** that can decay if the association becomes irrelevant

This is the computational basis for **associative memory** — the ability to recall related information without explicit prompting.

---

## Why Hebbian Learning for Automatic Linking?

### The Problem with Manual Graph Construction

Traditional knowledge graphs require:
1. **Named Entity Recognition (NER)** to extract entities from text
2. **Relation extraction** to identify relationships
3. **Entity linking** to resolve references (e.g., "John" → "John Smith")
4. **Schema design** to define entity types and relation types

This approach has fundamental issues for agent memory:

| Issue | Why it matters |
|-------|---------------|
| **Brittle NER** | Fails on domain-specific terms, slang, typos, creative language |
| **Schema lock-in** | Requires predefined entity types; can't adapt to new concepts |
| **Overfitting** | Extracts "entities" that aren't actually important for recall |
| **Latency** | NER + relation extraction adds API calls and processing time |
| **Cost** | Often requires LLM calls for quality extraction |

### The Hebbian Alternative

Hebbian learning sidesteps these problems by learning from **usage patterns** instead of structure:

- **No NER needed** — associations form organically from co-retrieval
- **Schema-free** — any memory can link to any memory
- **Self-organizing** — important associations strengthen, irrelevant ones fade
- **Zero latency** — runs locally, no API calls
- **Emergent semantics** — meaning emerges from use, not extraction

**Key insight:** If two memories are repeatedly retrieved together, they are *functionally related* from the agent's perspective — regardless of whether an NER system would recognize entities in them.

---

## The Mathematics

### Co-Activation Tracking

For each memory pair `(i, j)`, maintain a co-activation counter:

```
C(i, j) = number of times i and j appeared together in recall results
```

When a recall query returns memories `{m₁, m₂, ..., mₖ}`, increment:

```
C(mₐ, mᵦ) += 1   for all pairs (a, b) where a < b
```

### Link Formation Threshold

A Hebbian link is created when co-activation exceeds a threshold `θ`:

```
if C(i, j) ≥ θ:
    create_link(i, j, strength=1.0)
```

Default threshold: `θ = 3` (requires 3 co-activations to form a link)

**Rationale:** 
- `θ = 1` would create too many spurious links (one-time coincidences)
- `θ = 2` is borderline; could form from accidental overlap
- `θ = 3` requires consistent co-retrieval, indicating genuine association
- Higher θ (e.g., 5) makes link formation too conservative

### Link Strength Decay

Once created, link strength decays exponentially when not reinforced:

```
S(t) = S₀ · e^(-λt)
```

Where:
- `S₀` = initial strength (1.0)
- `λ` = decay rate (default: 0.1/day)
- `t` = time since last reinforcement

**Reinforcement:** When memories `i` and `j` co-activate *after* a link exists:

```
S(i, j) ← min(S(i, j) + 0.2, 1.0)  # Boost strength, cap at 1.0
```

This implements **Hebbian plasticity**: "neurons that fire together, wire together; neurons that fire apart, wire apart."

### Integration with ACT-R Activation

Hebbian links provide **spreading activation** in the ACT-R retrieval model:

```
A = B + C + I

where:
  C = Σ W_j · ln(S_j)   (spreading activation from linked memories)
  W_j = attention weight (1/fan-out)
  S_j = strength of link from memory j
```

When recalling memory `i`, activation spreads to all memories linked to `i`, boosting their retrieval scores proportionally to link strength.

**Effect:** Retrieving "machine learning" memories automatically surfaces linked memories about "PyTorch" or "TensorFlow" even if they don't match the query keywords.

---

## Comparison with shodh-memory's Approach

[shodh-memory](https://github.com/Shikhar03Stark/shodh-memory) is another neuroscience-inspired memory system. It uses:

1. **Entity extraction** via LLM (GPT-3.5/4) to identify important terms
2. **Explicit graph construction** with entity nodes and typed relations
3. **Embeddings** for semantic retrieval

### Philosophical Differences

| | **shodh-memory** | **engram (Hebbian)** |
|---|---|---|
| **Graph construction** | Explicit (LLM extracts entities) | Implicit (usage patterns form links) |
| **Entity recognition** | Required (NER step) | Not needed (any memory can link) |
| **Schema** | Typed entities + relations | Schema-free associations |
| **Semantics** | Embedding-based similarity | Usage-based relatedness |
| **Cost** | LLM calls for entity extraction | Zero cost (local computation) |
| **Latency** | +200-500ms per add (LLM call) | <1ms per add (local counter) |
| **Dependencies** | LLM API (OpenAI, Anthropic, etc.) | None (pure math) |

### When to Use Each

**Use shodh-memory's approach when:**
- You need structured entity graphs for external consumption (e.g., export to Neo4j)
- You want typed relations (e.g., "works_at", "located_in")
- You have budget for LLM calls on every memory addition
- Your domain has well-defined entity types

**Use engram's Hebbian approach when:**
- Memory needs to run **offline** or **without API costs**
- You want **self-organizing associations** that adapt to usage
- Your domain has **creative language, slang, or evolving terminology**
- You prioritize **low latency** (real-time agent interactions)
- You want the graph to reflect **what the agent actually needs** (usage-driven) rather than what an NER model thinks is important

### Hybrid Potential

The two approaches are complementary:
- **Hebbian links** for automatic, usage-driven associations (default)
- **Optional entity extraction** for explicit structured knowledge when needed

Future engram versions could support:
```python
mem.add("Paris is the capital of France",
        entities=[("Paris", "city"), ("France", "country")],  # Manual entities
        hebbian=True)  # Still track co-activation for links
```

This gives both worlds: structured graph + emergent associations.

---

## How It Integrates with Existing Graph Search

Engram already has a **graph search** feature using manually-added entity links:

```python
mem.add("SaltyHall uses Supabase",
        entities=["SaltyHall", "Supabase"])
```

Hebbian learning extends this by automatically creating links **without manual entity tagging**.

### Search Pipeline

1. **FTS5 keyword search** → initial candidate set
2. **ACT-R activation scoring** → rank candidates
3. **Graph expansion** (if `graph_expand=True`):
   - Find entities linked to top candidates
   - Fetch memories linked to those entities (1-hop expansion)
   - Re-score with spreading activation
4. **Hebbian links** augment graph expansion:
   - Memories co-activated with candidates get boosted
   - No entity extraction needed — links form from usage

### Example

```python
# Add memories (no manual entities)
mem.add("Python is great for ML")
mem.add("PyTorch is my favorite ML framework")
mem.add("I prefer PyTorch over TensorFlow")

# Query multiple times
mem.recall("machine learning framework", limit=3)  # Gets PyTorch, TensorFlow
mem.recall("Python ML tools", limit=3)             # Gets Python, PyTorch
mem.recall("PyTorch", limit=3)                     # Gets PyTorch, Python

# After 3+ co-activations, Hebbian links form automatically:
# - "Python is great for ML" ↔ "PyTorch is my favorite ML framework"
# - "PyTorch is my favorite ML framework" ↔ "I prefer PyTorch over TensorFlow"

# Now, querying "Python" automatically surfaces PyTorch memories via spreading activation
results = mem.recall("Python programming", graph_expand=True)
# Returns: PyTorch memories (via Hebbian link) + Python memories (direct match)
```

**No entity extraction. No schema. No LLM calls. Pure usage-driven associations.**

---

## Configuration Options

```python
from engram import Memory

mem = Memory("./agent.db",
             hebbian_threshold=3,     # Co-activations needed to form link
             hebbian_decay_rate=0.1,  # Link strength decay (1/day)
             hebbian_boost=0.2)       # Strength increase on reinforcement

# Disable Hebbian learning entirely (use only manual entity links)
mem = Memory("./agent.db", hebbian_enabled=False)
```

### Tuning Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `hebbian_threshold` | 3 | Co-activations needed to create a link |
| `hebbian_decay_rate` | 0.1 | Daily decay rate for link strength (λ) |
| `hebbian_boost` | 0.2 | Strength increase on reinforcement |
| `hebbian_max_strength` | 1.0 | Cap on link strength |
| `hebbian_prune_threshold` | 0.05 | Remove links below this strength |

**Aggressive linking** (chatbot, high recall):
```python
mem = Memory("bot.db", hebbian_threshold=2, hebbian_decay_rate=0.05)
```

**Conservative linking** (task agent, focused memory):
```python
mem = Memory("worker.db", hebbian_threshold=5, hebbian_decay_rate=0.2)
```

**No decay** (researcher, archive everything):
```python
mem = Memory("research.db", hebbian_decay_rate=0.0)
```

---

## Implementation Notes

### Storage Schema

Hebbian links are stored in a separate table:

```sql
CREATE TABLE hebbian_links (
    source_id TEXT REFERENCES memories(id) ON DELETE CASCADE,
    target_id TEXT REFERENCES memories(id) ON DELETE CASCADE,
    strength REAL DEFAULT 1.0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_reinforced TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (source_id, target_id)
);
```

**Bidirectional links:** Links are symmetric — `(A, B)` implies `(B, A)` with same strength.

### Performance

- **Add:** O(1) — no processing, links form on recall
- **Recall:** O(k²) co-activation counting (k = result set size, typically 5-10)
- **Graph expansion:** O(L·M) where L = links per memory, M = memories per entity
- **Memory overhead:** ~40 bytes per link (negligible for <10K links)

For a typical agent with 10,000 memories and 1,000 Hebbian links:
- Storage: ~400KB for links
- Recall latency: +2-5ms for co-activation tracking

### Integration with Consolidation

Hebbian links decay during consolidation cycles:

```python
mem.consolidate()  # Decays link strength by e^(-λ·Δt)
```

Pruning removes weak links:

```python
mem.forget()  # Removes links below hebbian_prune_threshold
```

---

## Future Directions

### 1. Weighted Co-Activation
Currently, all co-activations count equally. Future versions could weight by:
- **Position** in result set (top results count more)
- **Confidence** (certain memories count more than vague ones)
- **Recency** (recent co-activations count more)

```
C(i, j) += α · position_weight(i, j) · confidence_weight(i, j)
```

### 2. Causal Direction via STDP (Spike-Timing-Dependent Plasticity)

> **Priority: HIGH — Key upgrade for causal reasoning integration**
> See also: `/Users/potato/clawd/projects/causal-agent/DESIGN.md`

**Problem:** Current Hebbian links are **symmetric** (A↔B). They encode correlation ("A and B are related") but not causation ("A causes B").

**Solution:** Borrow STDP from neuroscience — use **temporal ordering** to infer causal direction.

**Principle:**
- If memory A is consistently activated/stored **before** memory B → strengthen A→B (A might cause B)
- If A is consistently activated/stored **after** B → strengthen B→A
- If simultaneous → correlation only (keep symmetric link)

**Implementation in `consolidate()`:**

```python
def consolidate_causal(self):
    """During consolidation, check temporal ordering of co-activated pairs."""
    for link in self.hebbian_links:
        mem_a = self.get(link.source_id)
        mem_b = self.get(link.target_id)

        # Check temporal ordering across all co-activations
        a_before_b = count_temporal_order(mem_a, mem_b)  # A activated before B
        b_before_a = count_temporal_order(mem_b, mem_a)  # B activated before A

        if a_before_b > b_before_a * 2:  # A consistently precedes B
            # Create/strengthen causal link A→B
            self.store(
                content=f"CAUSAL: {mem_a.summary} → {mem_b.summary}",
                type="causal",
                importance=link.strength,
                metadata={
                    "cause_id": mem_a.id,
                    "effect_id": mem_b.id,
                    "cause": mem_a.content[:100],
                    "effect": mem_b.content[:100],
                    "confidence": a_before_b / (a_before_b + b_before_a),
                    "observations": a_before_b + b_before_a
                }
            )
        elif b_before_a > a_before_b * 2:  # B consistently precedes A
            # Create/strengthen causal link B→A
            self.store(
                content=f"CAUSAL: {mem_b.summary} → {mem_a.summary}",
                type="causal",
                metadata={"cause_id": mem_b.id, "effect_id": mem_a.id, ...}
            )
        # else: symmetric correlation, keep existing bidirectional link
```

**Example:**
```
Observation sequence over time:
  t1: "changed auth.py signature"        (A)
  t2: "3 downstream tests failed"        (B)
  t5: "changed auth.py signature"        (A)
  t6: "2 downstream tests failed"        (B)
  t9: "changed auth.py signature"        (A)
  t10: "4 downstream tests failed"       (B)

consolidate() detects:
  A precedes B: 3 times
  B precedes A: 0 times
  Ratio: 3:0 → strong causal signal

Auto-creates:
  type=causal memory: "changing auth.py signature → downstream tests fail"
  confidence: 1.0
  strength: 0.9 (from Hebbian link strength)
```

**Schema change — add direction to hebbian_links:**

```sql
ALTER TABLE hebbian_links ADD COLUMN direction TEXT DEFAULT 'bidirectional';
-- Values: 'bidirectional' | 'forward' (source→target) | 'backward' (target→source)

ALTER TABLE hebbian_links ADD COLUMN temporal_forward INTEGER DEFAULT 0;
-- Count of times source was activated before target

ALTER TABLE hebbian_links ADD COLUMN temporal_backward INTEGER DEFAULT 0;
-- Count of times target was activated before source
```

**Integration with two-layer causal architecture:**
- GID graph = structural causation (code analysis, static)
- Engram STDP = experiential causation (learned from observations, dynamic)
- When both agree (GID says A→B, Engram confirms A→B) → high confidence
- When they disagree → interesting signal for investigation

**This is the bridge between GID (structure) and Engram (experience).**

### 3. Link Type Inference (General)
Beyond causal, automatically infer other link types from co-activation patterns:
- **Attributive:** A describes B (factual relations)
- **Contrastive:** A and B co-occur but contradict (opinions)

```python
link_type = infer_relation_type(mem_A, mem_B, coactivation_pattern)
```

### 3. Hebbian Forgetting (Anti-Hebbian Learning)
"Neurons that fire apart, wire apart" — negative co-activation:

```
If A is recalled but B is NOT (despite existing link):
    S(A, B) -= 0.1  # Weaken link
```

This prunes spurious associations over time.

### 4. Multi-Hop Spreading Activation
Currently: 1-hop expansion (direct links only).
Future: N-hop expansion with decay:

```
Activation at hop h: A_h = A_0 · S^h  (strength decays with distance)
```

Enables "chain of thought" retrieval — A → B → C even if A and C never co-activated.

---

## References

- **Hebb, D.O.** (1949). *The Organization of Behavior*. Wiley. — Original Hebbian learning principle
- **Anderson, J.R.** (2007). *How Can the Human Mind Occur in the Physical Universe?* Oxford. — ACT-R spreading activation
- **Ramsauer et al.** (2020). "Hopfield Networks Is All You Need." ICLR 2021. — Modern Hopfield = transformer attention
- **Brea et al.** (2016). "Matching Recall and Storage in Sequence Learning with Spiking Neural Networks." *J. Neuroscience*. — Biological basis for link decay

---

## Summary

**Hebbian learning in engram:**
- ✅ **Automatic** — no manual entity tagging
- ✅ **Schema-free** — any memory can link to any memory
- ✅ **Usage-driven** — associations form from actual recall patterns
- ✅ **Zero cost** — no LLM calls, no API dependencies
- ✅ **Biologically grounded** — implements Hebb's principle from neuroscience
- ✅ **Self-organizing** — strong links persist, weak links decay
- ✅ **Graph-compatible** — works alongside manual entity links

**Core equation:**

```
"Neurons that fire together, wire together"
→ "Memories recalled together, link together"
```

This is associative memory, emergent from use — not imposed by schema.
