# Engram v2.0.0 — New Features

This document describes the new features added in v2.0.0:

## 1. Namespace Isolation

Multi-agent shared memory with namespaced store/recall:

```typescript
import { Memory } from 'neuromemory-ai';

const mem = new Memory('./shared.db');

// Add memories to different namespaces
mem.addToNamespace('Trading insight about oil prices', {
  type: 'factual',
  importance: 0.8,
  namespace: 'trading',
});

mem.addToNamespace('Engine performance metrics', {
  type: 'factual',
  importance: 0.7,
  namespace: 'engine',
});

// Recall from specific namespace
const results = mem.recallFromNamespace('oil', 10, {
  namespace: 'trading',
});

// Recall from all namespaces
const allResults = mem.recallFromNamespace('performance', 10, {
  namespace: '*',
});
```

## 2. Emotional Bus

Connects memory to agent workspace files (SOUL.md, HEARTBEAT.md, IDENTITY.md) via feedback loops:

```typescript
import { Memory } from 'neuromemory-ai';

// Create memory with Emotional Bus
const mem = Memory.withEmotionalBus('./engram.db', './workspace');

// Get drives from SOUL.md
const drives = mem.emotionalBus!.getDrives();

// Add memory with emotional tracking
const memId = mem.addWithEmotion('Great debugging session today', {
  type: 'episodic',
  importance: 0.6,
  namespace: 'coding',
  emotion: 0.9,        // Positive valence
  domain: 'debugging',  // Track by domain
});

// Get emotional trends
const trends = mem.emotionalBus!.getTrends();
// → [{ domain: 'debugging', valence: 0.9, count: 1, ... }]

// Get SOUL update suggestions
const soulUpdates = mem.emotionalBus!.suggestSoulUpdates();
// → Suggests drive updates based on accumulated patterns

// Log behavior outcomes
mem.emotionalBus!.logBehavior('check_email', true);
mem.emotionalBus!.logBehavior('check_email', false);

// Get behavior statistics
const stats = mem.emotionalBus!.getBehaviorStats();
// → [{ action: 'check_email', total: 2, positive: 1, score: 0.5, ... }]

// Get HEARTBEAT update suggestions
const heartbeatUpdates = mem.emotionalBus!.suggestHeartbeatUpdates();
// → Suggests deprioritizing or boosting actions based on success rates
```

### Drive Alignment

Memories that align with SOUL drives automatically get importance boosts:

```typescript
// SOUL.md contains:
// curiosity: Always seek to understand new things
// helpfulness: Assist the user effectively

const aligned = 'I want to understand and learn new concepts';
const boost = mem.emotionalBus!.alignImportance(aligned);
// → boost = 1.5 (ALIGNMENT_BOOST constant)

const score = mem.emotionalBus!.alignmentScore(aligned);
// → score = 0.8 (high alignment with "curiosity" drive)

const foundDrives = mem.emotionalBus!.findAligned(aligned);
// → [['curiosity', 0.8], ...]
```

## 3. Access Control Lists (ACL)

Grant/revoke/check permissions for cross-agent access:

```typescript
import { Memory, Permission } from 'neuromemory-ai';

const mem = new Memory('./shared.db');
mem.setAgentId('ceo');  // Identify this agent

// Grant permissions
mem.grant('trader', 'trading', Permission.WRITE);
mem.grant('analyst', 'trading', Permission.READ);
mem.grant('admin', '*', Permission.ADMIN);  // Wildcard for all namespaces

// Check permissions
const canRead = mem.checkPermission('trader', 'trading', Permission.READ);
// → true (WRITE implies READ)

const canAdmin = mem.checkPermission('trader', 'trading', Permission.ADMIN);
// → false

// List permissions
const permissions = mem.listPermissions('admin');
// → [{ agentId: 'admin', namespace: '*', permission: 'admin', ... }]

// Revoke permission
mem.revoke('trader', 'trading');
```

## 4. Cross-Agent Subscriptions

Subscribe to namespaces and receive notifications:

```typescript
import { Memory } from 'neuromemory-ai';

const mem = new Memory('./shared.db');

// Subscribe to high-importance memories in trading namespace
mem.subscribe('ceo', 'trading', 0.8);

// Add a memory that exceeds the threshold
mem.addToNamespace('Oil price spike detected', {
  type: 'factual',
  importance: 0.9,
  namespace: 'trading',
});

// Check for notifications
const notifs = mem.checkNotifications('ceo');
// → [{ memoryId: '...', namespace: 'trading', importance: 0.9, content: '...', ... }]

// Check again - cursor updated, no duplicates
const notifs2 = mem.checkNotifications('ceo');
// → []

// Peek without updating cursor
const peekNotifs = mem.peekNotifications('ceo');
// → Same notifications, cursor not updated

// Subscribe to all namespaces
mem.subscribe('ceo', '*', 0.9);

// Unsubscribe
mem.unsubscribe('ceo', 'trading');

// List subscriptions
const subs = mem.listSubscriptions('ceo');
// → [{ subscriberId: 'ceo', namespace: '*', minImportance: 0.9, ... }]
```

## Workspace File Formats

### SOUL.md

```markdown
# Core Drives
curiosity: Always seek to understand new things
helpfulness: Assist the user effectively

# Values
- Be honest and direct
- Learn from mistakes
```

### HEARTBEAT.md

```markdown
# Tasks
- [ ] Check emails
- [x] Review calendar
- [ ] Run consolidation
```

### IDENTITY.md

```
name: TestAgent
creature: AI Assistant
vibe: helpful and curious
emoji: 🤖
```

## Backward Compatibility

All v1 APIs continue to work:

```typescript
// v1 API - still works
const mem = new Memory('./engram.db');
const id = mem.add('Test memory', { type: 'factual', importance: 0.5 });
const results = mem.recall('test', { limit: 5 });
mem.consolidate(1.0);
const stats = mem.stats();

// v2 API - new features
const id2 = mem.addToNamespace('Namespaced memory', {
  type: 'factual',
  namespace: 'trading',
});
const results2 = mem.recallFromNamespace('query', 5, {
  namespace: 'trading',
});
```

## Migration Guide

1. **Existing databases are auto-migrated**: The `namespace` column is added automatically with default value `'default'`.
2. **ACL tables are created on first use**: No manual migration needed.
3. **Emotional Bus is opt-in**: Use `Memory.withEmotionalBus()` to enable it.
4. **Subscriptions work out of the box**: Just call `subscribe()` on any Memory instance.

## TypeScript Types

All v2 features are fully typed:

```typescript
import {
  Memory,
  Permission,
  AclEntry,
  Subscription,
  Notification,
  EmotionalBus,
  SoulUpdate,
  HeartbeatUpdate,
  Drive,
  HeartbeatTask,
  Identity,
  EmotionalTrend,
  ActionStats,
  BehaviorLog,
  RecallResult,
  CrossLink,
} from 'neuromemory-ai';
```

## Testing

All 71 tests pass, including 16 new v2 tests covering:
- Namespace isolation
- ACL permission management
- Emotional Bus drive alignment
- Emotion tracking and trends
- Behavior feedback and suggestions
- Subscription and notification system
- Workspace file reading and writing
- Backward compatibility with v1 API

Run tests:
```bash
npm test
```

## License

AGPL-3.0-or-later (same as v1)
