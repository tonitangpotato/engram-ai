# Engram Memory System - Complete Workflow

This document explains how the Engram memory system works compared to traditional file-based memory, with detailed workflows and diagrams.

## Table of Contents

- [Current File-Based Memory](#current-file-based-memory)
- [Engram Memory System](#engram-memory-system)
- [Complete Workflow](#complete-workflow)
- [Comparison](#comparison)
- [Real-World Scenarios](#real-world-scenarios)
- [Architecture Flowchart](#architecture-flowchart)

---

## Current File-Based Memory

```
Storage:
MEMORY.md              → Manual write, manual read
memory/YYYY-MM-DD.md   → Daily notes
.gid/graph.yml         → Structured relationships
```

### Workflow

1. **Session start** → Read MEMORY.md + recent daily notes
2. **Learn something important** → Manually write to file
3. **Heartbeat** → Organize, update MEMORY.md
4. **Search** → grep / human recall "I think I wrote it somewhere"

### Problems

- ❌ No automatic decay (old unimportant info stays forever)
- ❌ No associations (mentioning A doesn't automatically recall related B)
- ❌ High manual maintenance cost

---

## Engram Memory System

### Storage Structure

```
agent.db (SQLite)
├── memories        → All memories (content, type, importance, activation)
├── access_log      → Access history (for calculating activation)
├── hebbian_links   → Associations (automatically formed)
└── fts_memories    → Full-text search index (FTS5)
```

### Core Concepts

| Concept | Description |
|---------|-------------|
| **Activation** | How "ready" a memory is for retrieval (recency × frequency × importance) |
| **Hebbian Links** | "Neurons that fire together wire together" - automatic associations |
| **Memory Layers** | working → core → archive (like human short/long-term memory) |
| **Consolidation** | Transfer important memories from working to core (like sleep) |

---

## Complete Workflow

### 1️⃣ Adding Memories

```python
mem.add("potato prefers Rust for coding", 
        type="preference", 
        importance=0.8)
```

**What happens:**
- ✓ Stored in SQLite `memories` table
- ✓ FTS5 index automatically updated
- ✓ Initial activation = 1.0
- ✓ Layer = "working" (new memories start here)

### 2️⃣ Recall During Conversation

```python
results = mem.recall("what language does potato use", limit=5)
# → Returns "potato prefers Rust for coding" (ranked by activation)
```

**Behind the scenes:**

```
┌─────────────────────────────────────────────────────────────┐
│                      RECALL PIPELINE                         │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  1. FTS5 Search                                              │
│     └─→ Find candidate memories matching keywords            │
│                                                              │
│  2. ACT-R Activation Calculation                             │
│     ├─→ Recently accessed? +score                            │
│     ├─→ Frequently accessed? +score                          │
│     └─→ High importance? +score                              │
│                                                              │
│  3. Hebbian Expansion                                        │
│     └─→ Find associated memories via co-activation links     │
│                                                              │
│  4. Rank & Return Top-K                                      │
│     └─→ Sort by activation, return best matches              │
│                                                              │
│  5. Record Access                                            │
│     └─→ Log this access → higher activation next time        │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 3️⃣ Hebbian Learning (Automatic)

When memories are frequently recalled together:

```python
# User asks about potato's tech preferences multiple times
mem.recall("potato programming")  # Returns: Rust, SaltyHall, gidterm
mem.recall("potato projects")     # Returns: SaltyHall, gidterm, Rust
mem.recall("potato languages")    # Returns: Rust, TypeScript
```

**Engram automatically forms associations:**

```
        ┌──────────────┐
        │ potato-rust  │
        └──────┬───────┘
               │ hebbian link (weight: 0.7)
               ▼
        ┌──────────────┐         ┌──────────────┐
        │  saltyhall   │◄───────►│   gidterm    │
        └──────────────┘         └──────────────┘
               │
               │ hebbian link (weight: 0.5)
               ▼
        ┌──────────────┐
        │ potato-js    │
        └──────────────┘
```

**Result:** Next time you ask "potato's tech stack", even without exact keyword match, Hebbian expansion finds related memories.

### 4️⃣ Consolidation — Like Sleep

```python
mem.consolidate()  # Run daily
```

**What happens:**

```
┌─────────────────────────────────────────────────────────────┐
│                    CONSOLIDATION                             │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Memory Promotion:                                           │
│  ┌──────────┐    high activation    ┌──────────┐            │
│  │ working  │ ──────────────────►   │   core   │            │
│  └──────────┘                       └──────────┘            │
│                                                              │
│  Memory Demotion:                                            │
│  ┌──────────┐    low activation     ┌──────────┐            │
│  │   core   │ ──────────────────►   │ archive  │            │
│  └──────────┘                       └──────────┘            │
│                                                              │
│  Activation Decay:                                           │
│  - All memories lose some activation (Ebbinghaus curve)      │
│  - Frequently accessed memories decay slower                 │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 5️⃣ Forgetting

```python
mem.forget(threshold=0.01)
```

- Deletes memories with activation < threshold
- Natural forgetting — no manual cleanup needed
- Keeps the database lean

### 6️⃣ Reward Learning

```python
mem.reward("Great answer!")  # Strengthens recently recalled memories
mem.reward("That's wrong")   # Weakens recently recalled memories
```

**Mechanism:**
- Tracks "eligibility traces" of recently recalled memories
- Positive feedback → boost activation
- Negative feedback → suppress activation
- Temporal discount (recent recalls affected more than older ones)

---

## Comparison

| Aspect | File System | Engram |
|--------|-------------|--------|
| **Storage** | Markdown files | SQLite database |
| **Recall** | Manual / grep | Semantic + activation ranking |
| **Associations** | Manual linking | Automatic Hebbian |
| **Forgetting** | Manual deletion | Automatic decay |
| **Consolidation** | Manual organization | consolidate() |
| **Human Readable** | ✅ Yes | ❌ Needs tools |
| **Git Friendly** | ✅ Yes | ❌ Binary diff |
| **Scalability** | ~1000s entries | ~100,000s entries |
| **Query Speed** | O(n) grep | O(log n) indexed |

---

## Real-World Scenarios

### Morning Routine

```
8:00 AM — Agent starts

Agent: mem.recall("what should I do today")
       → Returns yesterday's tasks (high activation from recent discussion)
       → Returns recurring tasks (high activation from frequency)
       → Skips old completed tasks (low activation, decayed)
```

### Mid-Conversation Association

```
User: "Let's talk about Rust"

Agent: mem.recall("Rust")
       
       Direct matches:
       → "potato prefers Rust for coding"
       → "gidterm is written in Rust"
       
       Hebbian expansion:
       → "SaltyHall uses Rust backend" (co-recalled with Rust before)
       → "potato dislikes JavaScript" (associated via preference pattern)
```

### End of Day

```
11:00 PM — Consolidation runs

mem.consolidate()

Results:
├── "Important meeting tomorrow at 9am" 
│   └── working → core (high importance, accessed today)
├── "User mentioned liking coffee"
│   └── stays in working (low importance, wait for reinforcement)
├── "Random tangent about weather"
│   └── activation decays (probably forgotten in a week)
```

### One Week Later

```
mem.forget(threshold=0.01)

Forgotten:
├── "Random tangent about weather" (activation → 0.005)
├── "Mentioned a TV show once" (activation → 0.002)

Still remembered:
├── "potato prefers Rust" (activation: 0.85, reinforced often)
├── "Important project deadline" (activation: 0.72, high importance)
```

---

## Architecture Flowchart

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           ENGRAM MEMORY SYSTEM                               │
└─────────────────────────────────────────────────────────────────────────────┘

                              ┌─────────────┐
                              │   Agent     │
                              │  (Claude)   │
                              └──────┬──────┘
                                     │
                    ┌────────────────┼────────────────┐
                    │                │                │
                    ▼                ▼                ▼
             ┌──────────┐     ┌──────────┐     ┌──────────┐
             │  add()   │     │ recall() │     │ reward() │
             └────┬─────┘     └────┬─────┘     └────┬─────┘
                  │                │                │
                  ▼                ▼                ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              MEMORY API                                      │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │  ACT-R      │  │  Hebbian    │  │ Confidence  │  │Consolidation│        │
│  │ Activation  │  │  Learning   │  │  Scoring    │  │   Engine    │        │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘        │
│         │                │                │                │                │
│         └────────────────┴────────────────┴────────────────┘                │
│                                   │                                          │
└───────────────────────────────────┼──────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              STORAGE LAYER                                   │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐             │
│  │    memories     │  │   access_log    │  │  hebbian_links  │             │
│  │                 │  │                 │  │                 │             │
│  │ • id           │  │ • memory_id     │  │ • source_id     │             │
│  │ • content      │  │ • timestamp     │  │ • target_id     │             │
│  │ • type         │  │ • context       │  │ • weight        │             │
│  │ • importance   │  │                 │  │ • co_activations│             │
│  │ • activation   │  │                 │  │                 │             │
│  │ • layer        │  │                 │  │                 │             │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘             │
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────┐           │
│  │                      fts_memories (FTS5)                     │           │
│  │                   Full-text search index                     │           │
│  └─────────────────────────────────────────────────────────────┘           │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘


                         ┌─────────────────────────┐
                         │    DAILY LIFECYCLE      │
                         └─────────────────────────┘

    ┌────────┐      ┌────────┐      ┌────────┐      ┌────────┐
    │ Morning│      │  Day   │      │ Evening│      │ Night  │
    │        │ ───► │        │ ───► │        │ ───► │        │
    │recall()│      │add()   │      │reward()│      │consoli-│
    │        │      │recall()│      │        │      │date()  │
    └────────┘      └────────┘      └────────┘      └────────┘
         │               │               │               │
         ▼               ▼               ▼               ▼
    ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐
    │Get tasks│    │Learn new│    │Reinforce│    │Transfer │
    │for today│    │  info   │    │ correct │    │ working │
    │         │    │         │    │memories │    │ → core  │
    └─────────┘    └─────────┘    └─────────┘    └─────────┘


                         ┌─────────────────────────┐
                         │   MEMORY LAYER FLOW     │
                         └─────────────────────────┘

                              New Memory
                                  │
                                  ▼
                         ┌───────────────┐
                         │    WORKING    │  ← All new memories start here
                         │   (short-term)│
                         └───────┬───────┘
                                 │
                    ┌────────────┴────────────┐
                    │                         │
              high activation           low activation
              (important +              (unimportant or
               frequent)                 infrequent)
                    │                         │
                    ▼                         ▼
           ┌───────────────┐         ┌───────────────┐
           │     CORE      │         │   (decay)     │
           │  (long-term)  │         │               │
           └───────┬───────┘         └───────┬───────┘
                   │                         │
                   │                         ▼
             low activation            ┌───────────┐
             over time                 │  FORGET   │
                   │                   │ (deleted) │
                   ▼                   └───────────┘
           ┌───────────────┐
           │    ARCHIVE    │
           │ (rarely used) │
           └───────────────┘
```

---

## Quick Reference

### Python API

```python
from engram import Memory

# Initialize
mem = Memory("./agent.db")

# Core operations
mem.add(content, type="factual", importance=0.5)
mem.recall(query, limit=5)
mem.consolidate()
mem.forget(threshold=0.01)
mem.reward(feedback)

# Utilities
mem.stats()
mem.export()
mem.pin(memory_id)    # Prevent forgetting
mem.unpin(memory_id)
```

### CLI

```bash
# Add memory
neuromem add "Important fact" --type factual --importance 0.8

# Recall
neuromem recall "search query" --limit 10

# Maintenance
neuromem consolidate
neuromem forget --threshold 0.01
neuromem stats
```

### Memory Types

| Type | Use Case |
|------|----------|
| `factual` | Facts and knowledge |
| `episodic` | Events and experiences |
| `preference` | User preferences |
| `procedural` | How-to knowledge |
| `emotional` | Emotional moments |
| `opinion` | Beliefs and opinions |

---

## Links

- **GitHub**: https://github.com/tonitangpotato/neuromemory-ai
- **PyPI**: https://pypi.org/project/neuromemory-ai/
- **npm**: https://www.npmjs.com/package/neuromemory-ai
