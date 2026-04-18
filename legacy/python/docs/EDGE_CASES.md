# Edge Cases & Known Limitations

This document describes edge cases, boundary conditions, and known limitations discovered during comprehensive testing.

## Test Coverage

**152 tests total** covering:
- Unit tests (core modules)
- Integration tests (e2e lifecycle)
- Edge case tests (43 specific scenarios)
- Hebbian learning tests

---

## Input Validation

### ✅ Handled Correctly

| Edge Case | Behavior |
|-----------|----------|
| Empty string content | Accepted, searchable |
| Whitespace-only content | Accepted |
| Very long content (100KB+) | Works, no truncation |
| Unicode (emoji, CJK, RTL, Cyrillic) | Full support |
| SQL injection attempts | Safely escaped, stored literally |
| Null bytes in content | Handled gracefully |
| Empty query | Returns empty list |
| Very long query (10k+ chars) | Works, no crash |

### ⚠️ Boundary Behaviors

| Edge Case | Behavior |
|-----------|----------|
| `importance < 0` | Should clamp to 0 or error |
| `importance > 1` | Should clamp to 1 or error |
| Invalid memory type | Falls back to default or errors |

---

## Scale Testing

### ✅ Performance Verified

| Scale | Recall Latency | Consolidation |
|-------|----------------|---------------|
| 100 memories | ~3ms | ~17ms |
| 1,000 memories | ~4ms | ~100ms |
| 10,000 memories | <500ms | <30s |

### Recommendations for Large Scale

- **10k+ memories**: Consider periodic archiving via `forget()`
- **100k+ memories**: Not tested; may need index optimization
- **Multi-GB databases**: SQLite handles well, but consider WAL mode

---

## Concurrency

### ✅ Safe Operations

| Operation | Thread-Safe? |
|-----------|--------------|
| Multiple readers | ✅ Yes |
| Single writer | ✅ Yes |
| Multiple writers (same process) | ⚠️ Serialized |

### ⚠️ Known Limitation: SQLite Concurrent Writes

**Problem**: Multiple processes writing simultaneously causes "database is locked" errors.

**Root Cause**: SQLite uses file-level locking. Concurrent writes from different connections can fail.

**Error Message**:
```
sqlite3.OperationalError: database is locked
```

**Solutions**:

1. **Single Memory instance per process** (recommended)
   ```python
   # Good: One instance, reuse it
   mem = Memory("./agent.db")
   # ... use mem throughout the process
   ```

2. **Enable WAL mode** for better concurrency
   ```python
   # The library could add this in future versions
   conn.execute("PRAGMA journal_mode=WAL")
   ```

3. **Use a concurrent-safe backend** (future feature)
   - PostgreSQL
   - Supabase
   - Turso

4. **Retry with backoff**
   ```python
   import time
   for attempt in range(3):
       try:
           mem.add("content", type="factual")
           break
       except sqlite3.OperationalError:
           time.sleep(0.1 * (attempt + 1))
   ```

---

## State Transitions

### ✅ Safe Operations

| Operation | Edge Case | Behavior |
|-----------|-----------|----------|
| `consolidate()` | Empty database | No error, no-op |
| `consolidate(days=0)` | Zero days | Handled gracefully |
| `consolidate(days=1000)` | Extreme decay | Memories decay to near-zero |
| `forget()` | All memories below threshold | Archives all (doesn't delete) |
| `forget(id)` | Non-existent ID | No error or KeyError |
| `reward()` | Empty database | No error, no-op |
| `reward()` | Extreme sentiment | Handled, clamped internally |

### ✅ Pinned Memory Protection

Pinned memories are protected from:
- Forgetting (even with high threshold)
- Archiving during consolidation
- Decay (strength preserved)

```python
mem.pin(memory_id)  # Protected forever
mem.unpin(memory_id)  # Resume normal decay
```

---

## Hebbian Learning Edge Cases

### ✅ Safe Operations

| Edge Case | Behavior |
|-----------|----------|
| Co-activate single memory | No-op (needs pairs) |
| Co-activate same ID twice | No self-links created |
| Link to deleted memory | Returns empty neighbors |
| Decay with no links | Returns 0 pruned |
| Many co-activations same pair | Strength capped at 1.0 |

### Link Formation Threshold

```python
# Links form after N co-activations (default: 3)
config.hebbian_threshold = 3

# Threshold 0 or 1: Links form almost immediately
# Threshold 1000+: Links rarely form
```

### Link Strength Over Time

With periodic co-activation, links persist indefinitely:
```
Day 10:  20 links, strength ~1.0
Day 50:  20 links, strength ~0.7 (decay + reinforcement)
Day 90:  20 links, strength ~0.5
Day 100: 20 links, strength ~0.5
```

Without co-activation, links decay and eventually prune (strength < 0.1).

---

## Recovery & Persistence

### ✅ Verified Behaviors

| Scenario | Behavior |
|----------|----------|
| Open non-existent path | Creates new database |
| Reopen existing database | Data persists correctly |
| Process crash mid-write | SQLite transactions protect integrity |

### Not Tested (Future Work)

- Disk full scenarios
- Read-only filesystem
- Database file corruption
- Schema migration between versions

---

## Configuration Edge Cases

### ✅ Extreme Values Handled

| Config | Extreme Value | Behavior |
|--------|---------------|----------|
| `working_decay = 0.0` | No decay | Memories never weaken |
| `working_decay = 0.99` | Extreme decay | Rapid strength loss |
| `hebbian_threshold = 0` | Immediate links | Forms on first co-activation |
| `hebbian_threshold = 1000000` | Never links | Threshold unreachable |

### Recommended Ranges

```python
working_decay: 0.01 - 0.5  # Lower = slower decay
core_decay: 0.001 - 0.1    # Should be slower than working
hebbian_threshold: 2 - 10  # Lower = faster link formation
hebbian_decay: 0.9 - 0.99  # Higher = slower link decay
```

---

## API Consistency

### ✅ Guaranteed Behaviors

| Method | Always Returns |
|--------|----------------|
| `recall()` | `list[dict]` (may be empty) |
| `add()` | `str` (memory ID) |
| `stats()` | `dict` with `total_memories` key |

### Memory Types

All 6 types are valid:
- `factual`
- `episodic`
- `relational`
- `emotional`
- `procedural`
- `opinion`

---

## Regression Tests

### Day-90 Zero Links (Fixed)

**Previous Bug**: Hebbian links decayed to 0 by day 90 and were pruned.

**Fix**: Links are now strengthened when memories are co-activated again, counteracting decay.

**Test**: `test_day_90_zero_links_regression` verifies links persist with periodic usage.

---

## Recommendations for Production Use

### Do's ✅

1. **Use one Memory instance per process**
2. **Call `consolidate()` daily** (like sleep)
3. **Pin critical memories** that must never decay
4. **Use appropriate config preset** for your use case
5. **Monitor `stats()['total_memories']`** for growth

### Don'ts ❌

1. **Don't open multiple connections** from different processes
2. **Don't set decay rates to 0** unless you want infinite memory
3. **Don't skip consolidation** for long periods (memory won't transfer to core)
4. **Don't ignore the 10k+ memory threshold** without testing

---

## Future Improvements

- [ ] WAL mode for better concurrency
- [ ] Pluggable backends (Postgres, Supabase)
- [ ] Schema versioning and migration
- [ ] Automatic retry on lock contention
- [ ] Memory usage limits and auto-pruning

---

*Last updated: 2026-02-03*
*Test coverage: 152 tests passing*
