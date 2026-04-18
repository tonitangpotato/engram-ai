# Engram v2 Migration Guide

## What's New in v2?

Engram v2 adds powerful features for building **multi-agent systems** with shared memory, emotional feedback, and cross-agent intelligence, while maintaining 100% backward compatibility with v1.

### Key Features

1. **Namespace Isolation** — Separate memory spaces for different agents/domains
2. **ACL (Access Control)** — Fine-grained permissions (read/write/admin)
3. **Subscriptions** — Pub/sub notifications for cross-agent coordination
4. **Emotional Bus** — Memory ↔ personality feedback loop

## Backward Compatibility

**All v1 code works unchanged in v2.** The default namespace is "default", so existing code behavior is identical.

```python
# This v1 code still works exactly the same in v2
from engram import Memory

mem = Memory("./agent.db")
mem.add("User prefers detailed explanations", type="relational")
results = mem.recall("user preferences")
```

## Upgrading Existing Databases

Engram v2 automatically migrates v1 databases by adding the `namespace` column with default value "default". No manual migration needed.

## Using v2 Features

### 1. Namespace Isolation

```python
from engram import Memory

memory = Memory("./shared.db")

# Store in different namespaces
memory.add_to_namespace(
    "Oil prices spiked 15%",
    type="factual",
    importance=0.9,
    namespace="trading"
)

memory.add_to_namespace(
    "CPU temperature high",
    type="factual",
    importance=0.6,
    namespace="engine"
)

# Query specific namespace
results = memory.recall_from_namespace(
    "market news",
    namespace="trading",
    limit=5
)

# Query all namespaces
results = memory.recall_from_namespace(
    "any topic",
    namespace="*",
    limit=10
)
```

### 2. Access Control

```python
# Set agent identity
memory.set_agent_id("trading_agent")

# Grant permissions (use as_system=True for initial setup)
memory.grant("analyst_agent", "trading", "read", as_system=True)
memory.grant("ceo_agent", "*", "admin", as_system=True)

# Revoke permissions
memory.revoke("analyst_agent", "trading")
```

### 3. Subscriptions & Notifications

```python
# CEO agent subscribes to all high-importance events
memory.subscribe("ceo_agent", "*", min_importance=0.8)

# Check for notifications
notifications = memory.check_notifications("ceo_agent")

for notif in notifications:
    print(f"[{notif['namespace']}] {notif['content']}")
    print(f"  Importance: {notif['importance']}")
```

### 4. Emotional Bus

```python
from engram import Memory
from pathlib import Path

# Create workspace files
workspace = Path("./workspace")
workspace.mkdir(exist_ok=True)

(workspace / "SOUL.md").write_text("""
# Core Drives
curiosity: Always seek to understand new concepts
efficiency: Prefer quick wins over lengthy investigations
""")

(workspace / "HEARTBEAT.md").write_text("""
# Daily Tasks
- [ ] Check emails
- [ ] Review pull requests
""")

# Create memory with Emotional Bus
memory = Memory.with_emotional_bus(
    db_path="./agent.db",
    workspace_dir=str(workspace)
)

# Get bus reference
bus = memory.emotional_bus()

# Store with emotion tracking
memory.add_with_emotion(
    "Debugging took 3 hours with no progress",
    type="episodic",
    emotion=-0.8,  # Negative experience
    domain="debugging"
)

# Check emotional trends
trends = bus.get_trends()
for trend in trends:
    print(trend.describe())

# Get SOUL update suggestions
suggestions = bus.suggest_soul_updates()
for s in suggestions:
    print(f"[{s.action}] {s.content}")

# Log behavior outcomes
bus.log_behavior("check_emails", True)  # Success
bus.log_behavior("auto_summarize", False)  # Failure

# Get HEARTBEAT suggestions
heartbeat_suggestions = bus.suggest_heartbeat_updates()
for s in heartbeat_suggestions:
    print(f"[{s.suggestion}] {s.action}")
```

## New Modules

### `engram.acl`

- `AclManager` — Manage permissions
- `Permission` enum — READ, WRITE, ADMIN
- `AclEntry` — Permission entry dataclass

### `engram.subscriptions`

- `SubscriptionManager` — Manage subscriptions
- `Subscription` — Subscription entry
- `Notification` — Notification event

### `engram.bus`

**Components:**
- `EmotionalBus` — Main bus interface
- `EmotionalAccumulator` — Track emotional trends
- `BehaviorFeedback` — Log action outcomes
- `Drive` — SOUL drive representation
- `parse_soul()`, `parse_heartbeat()`, `parse_identity()` — Workspace file parsers

**Data types:**
- `EmotionalTrend` — Domain emotional trend
- `ActionStats` — Behavior statistics
- `SoulUpdate` — Suggested SOUL change
- `HeartbeatUpdate` — Suggested HEARTBEAT change

## Testing

Run the comprehensive v2 test suite:

```bash
pytest tests/test_v2.py -v
```

Run all tests (including v1):

```bash
pytest tests/ -v -k "not concurrent"
```

## Examples

See:
- `examples/v2_multi_agent.py` — Namespace isolation + ACL + subscriptions
- `examples/v2_emotional_bus.py` — Drive alignment + emotional tracking + SOUL/HEARTBEAT

## Performance Notes

- Namespace queries are indexed (no performance impact)
- ACL checks are O(1) lookups
- Emotional trend tracking uses running averages (constant memory)
- Behavior logs can be pruned periodically

## Migration Checklist

- [ ] Update to `engramai==2.0.0`
- [ ] Test existing code (should work unchanged)
- [ ] Identify multi-agent use cases
- [ ] Plan namespace structure
- [ ] Set up workspace files for Emotional Bus (if needed)
- [ ] Update documentation with v2 features
- [ ] Run test suite

## Questions?

Check:
- README.md for feature overview
- CHANGELOG.md for detailed changes
- `tests/test_v2.py` for usage examples
- Examples in `examples/v2_*.py`
