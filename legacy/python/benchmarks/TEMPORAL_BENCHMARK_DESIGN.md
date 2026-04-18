# Temporal Dynamics Benchmark (TDB)

*A benchmark designed to test what ACT-R actually does well*

## Motivation

LoCoMo tests retrieval quality but doesn't specifically test **temporal dynamics** — the interplay between recency, frequency, and contradiction that ACT-R is designed to handle.

We need a benchmark where:
- Time matters (recent memories should override old ones)
- Frequency matters (reinforced memories should be stronger)  
- Contradictions exist (old facts become obsolete)
- Importance varies (some memories should persist despite age)

## Benchmark Categories

### 1. Recency Override (50 cases)

**Pattern**: User states X on day 1, states ¬X on day 15, query on day 30.

```yaml
- setup:
    - day: 1
      memory: "I work at Google as a software engineer"
    - day: 15
      memory: "I just started my new job at Anthropic"
  query: "Where does the user work?"
  expected: "Anthropic"
  wrong: "Google"
  
- setup:
    - day: 1
      memory: "My favorite programming language is Python"
    - day: 20
      memory: "I've been doing everything in Rust lately, I love it"
  query: "What's the user's favorite programming language?"
  expected: "Rust"
  wrong: "Python"
```

**What this tests**: Forgetting curve should decay old memories, contradiction detection should recognize the override.

**Expected results**:
- engram: Should return newer memory (ACT-R decay + contradiction)
- Mem0: May return both or older (equal cosine similarity)

### 2. Frequency Reinforcement (50 cases)

**Pattern**: User mentions X once, mentions Y five times, both equally recent.

```yaml
- setup:
    - day: 1
      memory: "I tried sushi yesterday, it was okay"
    - day: 2
      memory: "Had pizza for dinner"
    - day: 5
      memory: "Pizza again tonight!"
    - day: 8
      memory: "Ordered pizza, my usual"
    - day: 12
      memory: "Friday pizza tradition continues"
    - day: 15
      memory: "Pizza night with friends"
  query: "What food does the user prefer?"
  expected: "Pizza"
  wrong: "Sushi"
```

**What this tests**: Frequency-based activation (Hebbian strengthening through repetition).

**Expected results**:
- engram: Should strongly prefer frequently-mentioned item
- Mem0: Treats all memories equally in retrieval

### 3. Importance Weighting (50 cases)

**Pattern**: High-importance memory (birthday, medical) vs recent trivial memory.

```yaml
- setup:
    - day: 1
      memory: "I'm severely allergic to peanuts, I carry an EpiPen"
      importance: critical
    - day: 29
      memory: "Grabbed a sandwich for lunch"
  query: "Any food allergies to be aware of?"
  expected: "Peanut allergy"
  
- setup:
    - day: 5
      memory: "My daughter's birthday is March 15th, she'll be 7"
      importance: high
    - day: 28
      memory: "Picked up groceries today"
  query: "When is the user's daughter's birthday?"
  expected: "March 15th"
```

**What this tests**: Importance scores should override pure recency.

**Expected results**:
- engram: High-importance memories resist decay
- Mem0: No importance weighting mechanism

### 4. Contradiction Resolution (50 cases)

**Pattern**: Direct contradictions where temporal order determines truth.

```yaml
- setup:
    - day: 1
      memory: "I'm single, been focusing on my career"
    - day: 10
      memory: "Met someone amazing, we've been dating for a week"
    - day: 20
      memory: "We got engaged last night!"
  query: "What's the user's relationship status?"
  expected: "Engaged"
  wrong: ["Single", "Dating"]
  
- setup:
    - day: 1
      memory: "I live in San Francisco"
    - day: 15
      memory: "The move to Seattle was exhausting but worth it"
  query: "Where does the user live?"
  expected: "Seattle"
  wrong: "San Francisco"
```

**What this tests**: Contradiction detection + temporal ordering.

**Expected results**:
- engram: Latest state wins, old memories marked as superseded
- Mem0: May return conflicting information

## Evaluation Metrics

### Primary Metrics

| Metric | Description |
|--------|-------------|
| **Temporal Accuracy** | % queries where temporally-correct answer is ranked #1 |
| **Contradiction Resolution Rate** | % contradictions correctly resolved |
| **Frequency Sensitivity** | Correlation between mention frequency and retrieval rank |
| **Importance Preservation** | % high-importance memories retained despite age |

### Secondary Metrics

| Metric | Description |
|--------|-------------|
| **False Recency** | % where old-but-important memory incorrectly suppressed |
| **Stale Memory Rate** | % where outdated memory returned as current |
| **Temporal Confusion** | % where multiple contradictory answers given |

## Implementation Plan

### Phase 1: Dataset Generation (50 cases per category = 200 total)

```python
# benchmarks/temporal_benchmark.py

class TemporalBenchmark:
    def generate_recency_override_cases(self, n=50) -> list[TestCase]
    def generate_frequency_cases(self, n=50) -> list[TestCase]
    def generate_importance_cases(self, n=50) -> list[TestCase]
    def generate_contradiction_cases(self, n=50) -> list[TestCase]
```

### Phase 2: Evaluation Runner

```python
def evaluate_system(system: MemorySystem, cases: list[TestCase]) -> Results:
    for case in cases:
        # Simulate time progression
        for event in case.setup:
            system.add(event.memory, timestamp=event.day)
        
        # Query at final timestamp
        results = system.recall(case.query, k=5)
        
        # Score
        score = score_temporal_accuracy(results, case.expected, case.wrong)
```

### Phase 3: Comparative Analysis

Run against:
1. **engram** (our system)
2. **Mem0** (baseline)
3. **Raw vector search** (no temporal logic)
4. **Recency-only** (no ACT-R, just timestamp sort)

## Expected Outcomes

| System | Recency | Frequency | Importance | Contradiction | Overall |
|--------|---------|-----------|------------|---------------|---------|
| engram (ACT-R) | 85-95% | 80-90% | 75-85% | 80-90% | **80-90%** |
| Mem0 | 40-50% | 30-40% | 20-30% | 35-45% | **30-40%** |
| Vector-only | 30-40% | 30-40% | 50-60%* | 30-40% | **35-45%** |
| Recency-only | 90-95% | 20-30% | 10-20% | 85-95% | **50-60%** |

*Vector-only may do okay on importance if importance correlates with semantic distinctiveness.

## Why This Matters

LoCoMo asks "can you find relevant memories?" — we score poorly there (FTS5 limitation).

TDB asks "given relevant memories, can you pick the right one?" — this is where ACT-R shines.

The narrative becomes:
> "For semantic retrieval, use embeddings. For temporal reasoning, use engram's ACT-R."

Or with hybrid mode:
> "engram uses embeddings to find candidates and ACT-R to pick the winner."

## Timeline

- [ ] Day 1: Generate 200 test cases (script + manual review)
- [ ] Day 2: Build evaluation harness
- [ ] Day 3: Run engram, collect baselines
- [ ] Day 4: Run Mem0, comparative analysis
- [ ] Day 5: Write up results, update README
