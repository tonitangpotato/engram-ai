# HEARTBEAT.md — Memory Maintenance Schedule

## Memory Maintenance (every heartbeat)
- [ ] `mcporter call engram.consolidate` — run consolidation (working → core)
- [ ] Check engram stats, note any anomalies

## Weekly Maintenance
- [ ] `mcporter call engram.forget threshold=0.01` — prune weak memories
- [ ] `mcporter call engram.stats` — review memory health:
  - Working layer shouldn't grow unbounded (>500 = consolidate more)
  - Archive layer is fine to grow large
  - Check Hebbian link count is growing (means associations forming)

## Memory Habits (during conversations)
When answering questions about history/preferences:
→ First: `mcporter call engram.recall query="..." limit=5`

When learning something important:
→ Store: `mcporter call engram.store content="..." type=... importance=...`

When user gives positive/negative feedback:
→ `mcporter call engram.reward feedback="..."`
