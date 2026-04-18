# Agent Scenarios: Cognitive Memory in Practice

*How each cognitive mechanism maps to real AI agent use cases*

---

## Overview

engram's cognitive mechanisms aren't academic exercises—each solves a real problem that AI agents face in production.

```
Traditional RAG: "Find similar text"
engram: "Find the RIGHT memory for THIS moment"
```

---

## 1. ACT-R Base-Level Activation

### The Problem
Agent has 10,000 memories. User asks about "the project". Which project?

### Without ACT-R
- Returns all project-related memories with equal weight
- User gets confused by old, irrelevant projects
- Agent seems to have no sense of "current context"

### With ACT-R
```python
# Frequency: Project Alpha mentioned 50 times
# Recency: Last mentioned 2 hours ago
# → High activation, ranks first

# Project Beta: mentioned 5 times, 3 months ago
# → Low activation, ranks lower
```

### Real Agent Scenarios

| Agent Type | Scenario | ACT-R Benefit |
|------------|----------|---------------|
| **Coding Assistant** | "Continue with the refactor" | Knows which files were recently edited |
| **Personal Assistant** | "Schedule the meeting" | Knows which recurring meeting you mean |
| **Customer Support** | "What's my order status?" | Retrieves most recent order, not old ones |
| **Research Agent** | "Add to the literature review" | Knows which document is current |

### API Usage
```python
# ACT-R is automatic—just use recall()
results = mem.recall("the project")
# Recent + frequent projects rank first
```

---

## 2. Hebbian Learning ("Fire Together, Wire Together")

### The Problem
User always asks about "deployment" then immediately asks "which environment?"
Agent should learn this pattern.

### Without Hebbian
- Each query is independent
- Agent can't anticipate follow-up needs
- No learning from usage patterns

### With Hebbian
```python
# After 3+ co-retrievals:
# "deployment" ←→ "environment" link forms
# Now querying "deployment" also surfaces environment info
```

### Real Agent Scenarios

| Agent Type | Scenario | Hebbian Benefit |
|------------|----------|-----------------|
| **DevOps Agent** | Deploy → always needs env + secrets | Surfaces related info proactively |
| **Meeting Assistant** | Project mention → team members | Auto-includes relevant people |
| **Learning Agent** | Concept A → related Concept B | Builds knowledge connections |
| **Personal Agent** | "Mom" → "birthday March 15" | Learns personal associations |

### API Usage
```python
# Hebbian is automatic during recall
results = mem.recall("deployment")
# Related memories (env, secrets) get boost from Hebbian links

# Check what's linked
links = mem.hebbian_links(memory_id)
```

---

## 3. Ebbinghaus Forgetting Curve

### The Problem
Agent remembers a TODO from 6 months ago with same weight as today's TODO.

### Without Forgetting
- Old info clogs retrieval
- No sense of "staleness"
- Agent recommends outdated info confidently

### With Forgetting
```python
# Old TODO (6 months, never re-accessed)
retrievability = 0.02  # Effectively forgotten

# Recent TODO (yesterday, accessed twice)
retrievability = 0.95  # Fresh and strong

# Spaced repetition: important but old
# → stability grows, resists decay
```

### Real Agent Scenarios

| Agent Type | Scenario | Forgetting Benefit |
|------------|----------|-------------------|
| **Task Manager** | Old TODOs fade naturally | Reduces noise |
| **Knowledge Base** | Outdated docs rank lower | Fresh info first |
| **Learning Assistant** | Spaced repetition tracking | Optimal review scheduling |
| **News Agent** | Yesterday's news < today's | Temporal relevance |

### API Usage
```python
# Automatic in confidence scoring
results = mem.recall("TODO")
for r in results:
    print(r["confidence"])  # Lower for old, unaccessed memories
    
# Manual check
from engram.forgetting import retrievability
r = retrievability(entry)  # 0-1 probability of recall
```

---

## 4. Contradiction Detection (Retrieval-Induced Forgetting)

### The Problem
User said "I live in SF" (January), then "I moved to Seattle" (March).
Which is true?

### Without Contradiction Detection
- Both memories returned
- Agent gives conflicting info
- User loses trust

### With Contradiction Detection
```python
# March memory marks January memory as contradicted
entry.contradicted_by = march_memory_id

# January memory gets -3.0 activation penalty
# Seattle ranks first, SF doesn't surface
```

### Real Agent Scenarios

| Agent Type | Scenario | Contradiction Benefit |
|------------|----------|----------------------|
| **Personal Assistant** | Address/phone changes | Only current info surfaces |
| **Config Manager** | Setting changes | Latest config wins |
| **CRM Agent** | Customer status updates | Current status first |
| **Knowledge Agent** | Fact corrections | Corrected facts override |

### API Usage
```python
# Manual contradiction marking
mem.add("I live in Seattle", contradicts=old_sf_memory_id)

# Or automatic via update
mem.update(old_id, new_content="I live in Seattle")
```

---

## 5. Importance Weighting

### The Problem
User mentioned peanut allergy once (critical) and sandwich preference 10 times (trivial).
Frequency would bury the allergy.

### Without Importance
- Frequent = important (wrong!)
- Critical but rare info gets lost
- Dangerous in health/safety contexts

### With Importance
```python
# Allergy: importance=0.9, mentioned once
# → High activation despite low frequency

# Sandwich: importance=0.3, mentioned 10 times
# → Lower than allergy despite frequency
```

### Real Agent Scenarios

| Agent Type | Scenario | Importance Benefit |
|------------|----------|-------------------|
| **Health Assistant** | Allergies, medications | Safety info persists |
| **Legal Agent** | Contract deadlines | Critical dates don't fade |
| **Security Agent** | Access credentials | High-stakes info protected |
| **Personal Agent** | Birthdays, anniversaries | Important dates remembered |

### API Usage
```python
# Set importance at add time
mem.add("Allergic to peanuts - EpiPen in bag", importance=0.9)
mem.add("Had a sandwich for lunch", importance=0.2)

# Or update later
mem.update(id, importance=0.9)
```

---

## 6. Consolidation (Sleep Replay)

### The Problem
Agent has chatted all day. Working memory is full. Long-term patterns aren't forming.

### Without Consolidation
- All memories stay in working state
- No "learning" from patterns
- Memory store grows unbounded

### With Consolidation
```python
# After consolidation cycle:
# - Strong memories promoted to core
# - Weak memories demoted to archive
# - Hebbian links that weren't used decay
# - Global strength normalized
```

### Real Agent Scenarios

| Agent Type | Scenario | Consolidation Benefit |
|------------|----------|----------------------|
| **Long-running Agent** | 24/7 operation | Prevents memory bloat |
| **Learning Agent** | End of study session | Commits learnings to long-term |
| **Personal Agent** | Overnight "sleep" | Organizes day's memories |
| **Enterprise Agent** | Shift changes | Transfers context |

### API Usage
```python
# Run consolidation (e.g., at session end, nightly)
mem.consolidate(days=1.0)

# Or automatic in long-running agent
class Agent:
    def on_session_end(self):
        self.memory.consolidate()
```

---

## 7. Memory Types & Layers

### The Problem
Treating all memories the same—facts, events, skills, opinions—leads to wrong decay rates and retrieval strategies.

### Memory Types
```python
EPISODIC   # "On Feb 3 we discussed..."  → Fast decay
SEMANTIC   # "Python is a language"      → Slow decay
PROCEDURAL # "To deploy, run make..."    → Very slow decay
RELATIONAL # "potato prefers X"          → Medium decay
EMOTIONAL  # "User was frustrated"       → Affects importance
```

### Memory Layers
```python
L1_WORKING  # Just added, still forming
L2_RECENT   # Recent, consolidating
L3_CORE     # Long-term, stable
L4_ARCHIVE  # Forgotten but recoverable
```

### Real Agent Scenarios

| Agent Type | Memory Type Use |
|------------|-----------------|
| **Knowledge Base** | SEMANTIC for facts (slow decay) |
| **Personal Assistant** | EPISODIC for events (fast decay) |
| **Coding Agent** | PROCEDURAL for commands (persist) |
| **Relationship Agent** | RELATIONAL for preferences |

### API Usage
```python
from engram import MemoryType

mem.add("Python uses indentation", type=MemoryType.SEMANTIC)
mem.add("Yesterday's standup was long", type=MemoryType.EPISODIC)
mem.add("To deploy: git push origin main", type=MemoryType.PROCEDURAL)
```

---

## Summary: When Each Mechanism Helps

```
┌────────────────────────────────────────────────────────────────┐
│                    QUERY ARRIVES                                │
├────────────────────────────────────────────────────────────────┤
│                                                                │
│  "What's the current status?"                                  │
│       │                                                        │
│       ├─► ACT-R: Recent + frequent memories first              │
│       ├─► Contradiction: Old statuses suppressed               │
│       └─► Importance: Critical updates prioritized             │
│                                                                │
│  "Tell me about X and related things"                          │
│       │                                                        │
│       ├─► Hebbian: Pull in associated memories                 │
│       ├─► Spreading: Context-based expansion                   │
│       └─► Graph: Entity relationships                          │
│                                                                │
│  "Any safety concerns I should know?"                          │
│       │                                                        │
│       ├─► Importance: Critical info surfaces despite age       │
│       ├─► Forgetting: Trivial old info suppressed              │
│       └─► Types: PROCEDURAL safety rules persist               │
│                                                                │
│  [Long-running agent maintenance]                              │
│       │                                                        │
│       ├─► Consolidation: Working → Core transfer               │
│       ├─► Forgetting: Prune irretrievable memories             │
│       └─► Layers: Promote/demote based on strength             │
│                                                                │
└────────────────────────────────────────────────────────────────┘
```

---

## Quick Reference: Mechanism → Agent Benefit

| Mechanism | One-Liner Benefit |
|-----------|-------------------|
| **ACT-R Activation** | "Current context" awareness |
| **Hebbian Learning** | Learn usage patterns automatically |
| **Forgetting Curve** | Old info fades, fresh info wins |
| **Contradiction** | Handle updates/corrections gracefully |
| **Importance** | Critical info doesn't get buried |
| **Consolidation** | Long-term learning, prevent bloat |
| **Memory Types** | Right decay rate for right info |
