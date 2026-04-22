# Entity Indexing (B1) Task Graph

## Overview
ISS-009 implementation broken down into 7 executable tasks with clear dependencies.

## Task Breakdown

```
┌────────────────────────────────────────────────────────────────┐
│ b1-entity-index (B1)                                           │
│ Entity 索引实现 — extraction + storage + recall (ISS-009)      │
│ Status: todo | Priority: 20 | Ritual: true                     │
└────────────────────────────────────────────────────────────────┘
                            │
                            │ subtask_of (7 tasks)
                            ▼
        ┌─────────────────────────────────────────────────┐
        │                                                 │
        │  TASK 1: entity-types (B1.1)                   │
        │  ├─ Create src/entities.rs (~80 lines)         │
        │  ├─ Add EntityType, ExtractedEntity types      │
        │  ├─ Add EntityConfig to config.rs (+20)        │
        │  ├─ Add aho-corasick dep to Cargo.toml         │
        │  └─ Add pub mod entities to lib.rs (+2)        │
        │                                                 │
        └───────────────┬────────────────┬────────────────┘
                        │                │
         depends_on     │                │ depends_on
                        │                │
        ┌───────────────▼─────┐    ┌────▼──────────────────────┐
        │                     │    │                           │
        │  TASK 2:            │    │  TASK 3:                  │
        │  entity-unit-tests  │    │  entity-storage (B1.3)    │
        │  (B1.2)             │    │  ├─ storage.rs (+120)     │
        │  [PARALLEL]         │    │  ├─ upsert_entity()       │
        │  ├─ Test patterns   │    │  ├─ link_memory_entity()  │
        │  ├─ Test norm       │    │  ├─ find_entities()       │
        │  └─ ~100 lines      │    │  └─ 8 CRUD methods        │
        │                     │    │                           │
        └─────────┬───────────┘    └───────┬───────────────────┘
                  │                        │
                  │                        │ depends_on
                  │         ┌──────────────┴────────────────┐
                  │         │                               │
                  │    ┌────▼──────────────┐    ┌──────────▼────────────┐
                  │    │                   │    │                       │
                  │    │  TASK 4:          │    │  TASK 5:              │
                  │    │  entity-add-raw   │    │  entity-recall        │
                  │    │  (B1.4)           │    │  (B1.5)               │
                  │    │  ├─ memory.rs     │    │  ├─ entities.rs       │
                  │    │  │   (+80)        │    │  │   (+150)           │
                  │    │  ├─ Hook write    │    │  ├─ 4th hybrid        │
                  │    │  │   path         │    │  │   channel          │
                  │    │  └─ Extract +     │    │  └─ Entity-aware      │
                  │    │     link          │    │     retrieval         │
                  │    │                   │    │                       │
                  │    └────────┬──────────┘    └──────────┬────────────┘
                  │             │                          │
                  │             │ depends_on  depends_on   │
                  │             └──────────┬───────────────┘
                  │                        │
                  │                   ┌────▼──────────────────┐
                  │                   │                       │
                  │                   │  TASK 6:              │
                  │                   │  entity-backfill      │
                  │                   │  (B1.6)               │
                  │                   │  ├─ main.rs (+40)     │
                  │                   │  ├─ CLI subcommand    │
                  │                   │  └─ Process existing  │
                  │                   │     memories          │
                  │                   │                       │
                  │                   └────────┬──────────────┘
                  │                            │
                  │                            │ depends_on
                  │                            │
                  └──────────────┬─────────────▼───────────────┐
                                 │                             │
                                 │  TASK 7:                    │
                                 │  entity-integration-tests   │
                                 │  (B1.7)                     │
                                 │  ├─ tests/entity_*.rs       │
                                 │  │   (~150 lines)           │
                                 │  ├─ E2E entity indexing     │
                                 │  ├─ Test recall boost       │
                                 │  └─ Test backfill           │
                                 │                             │
                                 └─────────────────────────────┘
```

## Execution Order

### Phase 1: Foundation
1. **entity-types** - Must run first (foundational types)

### Phase 2: Parallel Development
2. **entity-unit-tests** - Can run in parallel with storage
3. **entity-storage** - CRUD operations

### Phase 3: Integration
4. **entity-add-raw** - Write path (depends on storage)
5. **entity-recall** - Read path (depends on storage)

### Phase 4: Tooling & Validation
6. **entity-backfill** - CLI command (depends on add-raw + recall)
7. **entity-integration-tests** - Final validation (depends on backfill)

## Key Metrics

| Metric | Value |
|--------|-------|
| Total Tasks | 7 |
| Total Estimated LOC | ~720 lines |
| New Files | 2 (entities.rs, entity_integration_test.rs) |
| Modified Files | 5 (storage.rs, memory.rs, config.rs, main.rs, lib.rs) |
| New Dependencies | 1 (aho-corasick) |
| Parallel Tasks | 1 (entity-unit-tests) |
| Critical Path Length | 5 (types → storage → add-raw → backfill → integration) |

## Files Created/Modified

### New Files
- `src/entities.rs` (~380 lines total)
  - Task 1: Types & EntityExtractor (~80 lines)
  - Task 2: Unit tests (~100 lines)
  - Task 5: entity_recall() (~150 lines)
  - Misc: ~50 lines
  
- `tests/entity_integration_test.rs` (~150 lines)
  - Task 7: E2E tests

### Modified Files
- `src/storage.rs` (+120 lines) - Task 3
- `src/memory.rs` (+100 lines) - Task 4 (+80), Task 5 (+20)
- `src/main.rs` (+40 lines) - Task 6
- `src/config.rs` (+20 lines) - Task 1
- `src/lib.rs` (+2 lines) - Task 1
- `Cargo.toml` (+1 dependency) - Task 1

## Dependencies Added
```toml
aho-corasick = "1.1"
```

## Success Criteria
- [ ] All 7 tasks completed
- [ ] All tests passing (unit + integration)
- [ ] Entity extraction working on add_raw()
- [ ] Entity-aware recall integrated as 4th channel
- [ ] Backfill command functional
- [ ] No performance regression (<5ms overhead per memory write)
- [ ] Existing functionality unchanged

## Related Issues
- ISS-009: Entity indexing implementation
- ISS-003: Memory deduplication (B2, depends on B1)
- ISS-007: Confidence scoring (B3, depends on B1)
