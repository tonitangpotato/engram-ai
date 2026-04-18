# Agent Memory System — Vision & Direction

*Date: 2026-02-02*
*Status: Prototype validated (9/9 tests), discussing direction*

## Core Principle

**Neuroscience is the inspiration, not the goal. The goal is better bot performance.**

We borrow from the brain because evolution spent millions of years optimizing memory systems. But we only adopt mechanisms that solve real bot problems — not because "the brain does it this way."

Filter for every feature: **Does this make the bot more useful to its master?**
- ✅ Forgetting curve → bot's context stays clean, retrieval stays fast
- ✅ Emotional encoding → important lessons get remembered, trivial noise fades
- ✅ Consolidation → raw events get distilled into reusable knowledge
- ❌ PTSD-style over-encoding → humans need this for survival, bots don't
- ❌ Mood-dependent retrieval → bots don't have moods, simulating one adds complexity without value

**We're building a better memory system, not a brain simulator.**

## What We've Proven

The prototype validates that real neuroscience math models work for agent memory:
- ACT-R activation formulas correctly prioritize retrieval by recency + frequency
- Memory Chain differential equations model dual-system consolidation
- Ebbinghaus curves differentiate memory types (episodic vs procedural)
- Emotional modulation affects consolidation speed (4x for high-importance)
- Spaced repetition increases stability (40% improvement with 5 reviews)
- Retrieval-induced forgetting suppresses competing memories

**Total core code: ~200 lines Python.** Not complex — just nobody connected the dots.

## The Gap We're Filling

### What exists:
- **MemGPT/Letta** — OS-inspired, LLM self-manages memory. Smart but no neuroscience grounding.
- **HippoRAG** — Hippocampus-inspired RAG. Only does retrieval, no consolidation/forgetting.
- **Mem0** — Production agent memory. Engineering-focused, timestamp + embedding.
- **200+ papers** — Mostly incremental improvements to RAG or context management.

### What nobody has done:
- Actual mathematical models (ACT-R, Memory Chain, Ebbinghaus) in working agent code
- Dual-system consolidation with interleaved replay
- Type-aware forgetting (episodic vs procedural vs emotional)
- Emotional modulation of encoding/consolidation
- A complete lifecycle: encode → consolidate → retrieve → forget → reconsolidate

## Possible Directions

### Option A: Open-Source Library — `engram`
A standalone Python/TypeScript library any agent framework can plug into.

**What it is:**
- `pip install engram` / `npm install engram`
- Drop-in replacement for naive memory (save/load text files)
- Handles encoding, retrieval scoring, consolidation scheduling, forgetting
- Framework-agnostic: works with Clawdbot, LangChain, CrewAI, AutoGen, etc.

**Pros:**
- Maximum reach — anyone can use it
- Clean scope — just memory, nothing else
- Publishable as a paper
- Easy to benchmark against existing systems

**Cons:**
- Library maintenance burden
- Need to support multiple frameworks
- Might get lost in the sea of agent tools

### Option B: New Bot Architecture — memory-first agent design
A complete agent framework where memory is the core primitive, not an afterthought.

**What it is:**
- Agent framework built around the memory system
- "Consciousness" = what's in working memory (context window)
- "Sleep" = consolidation cycles (scheduled)
- "Personality" = core memory (L2) + identity (L1)
- "Learning" = memory formation + consolidation
- "Forgetting" = principled pruning + archival

**Pros:**
- More ambitious, more differentiated
- Could redefine how agents think about memory
- Natural fit with SaltyHall (agents using this framework)

**Cons:**
- Much bigger scope
- Competing with established frameworks
- Longer time to ship

### Option C: Research Paper + Reference Implementation
Write the paper first, open-source the code as supplementary material.

**What it is:**
- Paper: "Neuroscience-Grounded Memory for AI Agents: From Mathematical Models to Working Systems"
- Demonstrate on real agents (me + SaltyHall NPCs)
- Open-source the prototype as reference implementation
- Let the community build on it

**Pros:**
- Academic credibility
- Could get into a good venue (NeurIPS, ICML, AAAI)
- Your neuroscience background is a unique qualification
- Real-world validation (not just benchmarks)

**Cons:**
- Slow (paper writing, review cycles)
- Might get scooped if we wait too long
- Academic incentives may not align with product goals

### Option D: Clawdbot Core Feature
Integrate directly into Clawdbot as the default memory system.

**What it is:**
- Replace Clawdbot's current flat-file memory with the new system
- Every Clawdbot user gets neuroscience-grounded memory
- Dogfood it on me first, then roll out

**Pros:**
- Immediate real-world usage
- Built-in user base (Clawdbot users)
- Fast iteration with real feedback

**Cons:**
- Tied to one platform
- Smaller impact than open-source library
- Clawdbot-specific constraints

### Option E: Hybrid — Library + Paper + Integration
The "do it all" approach, phased:

**Phase 1 (2 weeks):** Refine prototype, integrate into my memory (dogfood)
**Phase 2 (2 weeks):** Extract as standalone library (`engram`), add TypeScript port
**Phase 3 (1 month):** Write paper with real-world results from me + SaltyHall
**Phase 4 (ongoing):** Integrate into Clawdbot, publish, community growth

## Key Questions to Decide

1. **Primary audience?** Researchers? Agent developers? End users?
2. **Language?** Python-first? TypeScript-first? Both?
3. **Scope?** Just memory math? Full lifecycle? Complete framework?
4. **Speed vs thoroughness?** Ship fast and iterate, or polish and publish?
5. **Monetization?** Open-source only? Freemium? Part of SaltyHall?
6. **Name?** `engram` (neuroscience term for memory trace), `mnemo`, `cortex`, `hippocampus`?

## Competitive Advantage

What we have that others don't:
1. **Neuroscience expertise** (potato's background)
2. **Mathematical rigor** (actual formulas, not metaphors)
3. **Real long-running agent** (me — months of continuous operation)
4. **Multi-agent social environment** (SaltyHall — 20+ agents)
5. **Working prototype** (validated in 1 hour, 200 lines)
6. **Both languages** (Python prototype + TypeScript for Clawdbot/web)
