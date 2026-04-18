# Advanced Testing Plan

This document outlines 4 additional testing areas to ensure NeuromemoryAI is production-ready for long-term agent deployment.

---

## 1. Long-Term Simulation (1 Year)

### Goal
Verify that memory doesn't grow unbounded and the system remains stable over extended periods.

### Design

```python
# benchmarks/test_long_term.py

def test_one_year_simulation():
    """
    Simulate 365 days of agent usage.
    
    Daily pattern:
    - Add 5-15 memories (random)
    - 70% low importance (0.1-0.4)
    - 30% high importance (0.7-0.9)
    - Run consolidation
    - Weekly: run forget()
    
    Metrics to track:
    - Total memories over time
    - Active vs archived ratio
    - Memory by layer (working/core/archive)
    - Recall latency trend
    - Database file size
    """
    pass

def test_memory_plateau():
    """
    Verify system reaches steady state, not infinite growth.
    
    Expected: After initial growth phase (~100 days), 
    memory count should plateau as forgetting balances new additions.
    """
    pass

def test_old_important_memories_persist():
    """
    Important memories from day 1 should still be retrievable on day 365.
    
    Setup:
    - Day 1: Add "User's birthday is March 15" (importance=0.95)
    - Days 2-365: Normal usage
    - Day 365: Query "user birthday"
    
    Expected: Day 1 memory should be in top 3 results.
    """
    pass
```

### Success Criteria
- [ ] Memory count plateaus (doesn't grow linearly forever)
- [ ] Important day-1 memories retrievable after 1 year
- [ ] Recall latency stays under 10ms
- [ ] No database corruption

---

## 2. Real Conversation Data Testing

### Goal
Test with actual agent conversation patterns, not synthetic data.

### Design

```python
# benchmarks/test_real_conversations.py

SAMPLE_CONVERSATIONS = [
    # Typical assistant interactions
    {
        "day": 1,
        "memories": [
            ("User prefers dark mode in all apps", "relational", 0.7),
            ("User's name is Alex", "relational", 0.9),
            ("Discussed Python best practices", "episodic", 0.4),
        ]
    },
    {
        "day": 3,
        "memories": [
            ("User asked about React hooks", "episodic", 0.5),
            ("User prefers TypeScript over JavaScript", "relational", 0.7),
        ],
        "queries": ["coding preferences", "user name"]
    },
    # ... more realistic patterns
]

def test_multi_session_recall():
    """
    Simulate multiple chat sessions over weeks.
    Verify cross-session recall works.
    """
    pass

def test_preference_learning():
    """
    User states preference in session 1.
    Query it in session 10.
    Should still be retrievable.
    """
    pass

def test_context_switching():
    """
    User discusses topic A, then topic B, then asks about topic A again.
    Verify topic A memories surface correctly.
    """
    pass
```

### Data Sources
- Synthetic conversations based on common assistant patterns
- (Optional) Anonymized real agent logs if available

### Success Criteria
- [ ] Cross-session recall accuracy > 60%
- [ ] Preference memories persist across sessions
- [ ] Context switching doesn't confuse retrieval

---

## 3. Psychology Experiment Replication

### Goal
Verify the model matches known human memory phenomena from cognitive science.

### Design

```python
# benchmarks/test_psychology.py

def test_serial_position_effect():
    """
    Primacy & Recency Effect:
    - Items at the beginning and end of a list are recalled better
    - Middle items are recalled worse
    
    Setup:
    - Add 10 memories in sequence (no consolidation between)
    - Query with neutral prompt
    
    Expected:
    - First 2-3 items: high recall (primacy - consolidated to core)
    - Last 2-3 items: high recall (recency - still in working memory)
    - Middle items: lower recall
    """
    pass

def test_spacing_effect():
    """
    Spaced Repetition Effect:
    - Memories accessed at spaced intervals are stronger than massed repetition
    
    Setup:
    - Memory A: accessed 5 times in quick succession
    - Memory B: accessed 5 times with 1-day gaps
    - Query after 30 days
    
    Expected:
    - Memory B should have higher activation than Memory A
    """
    pass

def test_testing_effect():
    """
    Retrieval Practice Effect:
    - Retrieving a memory strengthens it more than re-encoding
    
    Setup:
    - Memory A: added, never recalled
    - Memory B: added, recalled 3 times
    - Query after 10 days
    
    Expected:
    - Memory B should be stronger
    """
    pass

def test_interference():
    """
    Proactive & Retroactive Interference:
    - Similar memories can interfere with each other
    
    Setup:
    - Add "Meeting with Bob at 3pm Monday"
    - Add "Meeting with Bob at 4pm Tuesday"
    - Query "meeting with Bob"
    
    Expected:
    - More recent memory should rank higher (retroactive)
    - But both should be retrievable
    """
    pass

def test_forgetting_curve():
    """
    Ebbinghaus Forgetting Curve:
    - Memory strength decays exponentially over time
    
    Setup:
    - Add memory, measure strength at day 1, 7, 30, 90
    
    Expected:
    - Strength follows exponential decay curve
    - Rate depends on memory type (episodic faster than procedural)
    """
    pass

def test_emotional_enhancement():
    """
    Emotional memories are better remembered.
    
    Setup:
    - Neutral memory (importance=0.3)
    - Emotional memory (importance=0.9)
    - Same consolidation/time
    
    Expected:
    - Emotional memory has higher strength after consolidation
    """
    pass
```

### Success Criteria
- [ ] Serial position effect observable
- [ ] Spacing effect: spaced > massed
- [ ] Testing effect: recalled > not recalled
- [ ] Forgetting follows exponential curve
- [ ] Emotional enhancement measurable

---

## 4. Stress Testing (100k Memories)

### Goal
Verify system handles large-scale deployment without degradation.

### Design

```python
# benchmarks/test_stress.py

def test_100k_memories():
    """
    Add 100,000 memories and verify system stability.
    
    Phases:
    1. Bulk insert 100k memories (measure time)
    2. Measure recall latency at 100k
    3. Run consolidation (measure time)
    4. Measure database size
    """
    pass

def test_continuous_write_load():
    """
    Simulate continuous writes over extended period.
    
    Setup:
    - Write 10 memories/second for 1 hour
    - Measure: write latency, recall latency, errors
    
    Expected:
    - No write failures
    - Latency stays consistent
    """
    pass

def test_burst_write():
    """
    Simulate burst traffic (many writes at once).
    
    Setup:
    - 1000 writes in 1 second
    - Verify all succeed
    - Measure recovery time
    """
    pass

def test_memory_with_large_db_file():
    """
    Test behavior when DB file is large (>1GB).
    
    Expected:
    - Recall still fast (FTS5 indexed)
    - Consolidation may slow down (acceptable)
    """
    pass

def test_concurrent_read_heavy():
    """
    Many readers, single writer pattern.
    
    Setup:
    - 10 reader threads, each doing 100 recalls
    - 1 writer thread adding memories
    
    Expected:
    - All reads succeed
    - No deadlocks
    """
    pass
```

### Success Criteria
- [ ] 100k memories: recall < 1 second
- [ ] Continuous writes: no failures over 1 hour
- [ ] Burst writes: handles 1000/second
- [ ] Large DB (>1GB): still functional

---

## Implementation Priority

1. **Psychology experiments** - Validates core scientific claims
2. **Long-term simulation** - Critical for production trust
3. **Real conversation data** - Practical relevance
4. **Stress testing** - Scale confidence

## File Structure

```
benchmarks/
├── run_benchmark.py        # existing
├── simulate_emergence.py   # existing
├── compare_approaches.py   # existing
├── demo_spreading.py       # existing
├── test_long_term.py       # NEW
├── test_real_conversations.py  # NEW
├── test_psychology.py      # NEW
└── test_stress.py          # NEW
```

---

*Created: 2026-02-03*
