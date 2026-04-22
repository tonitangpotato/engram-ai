# Entity Indexing (B1) - Task Graph Configuration Complete

## Summary

Successfully configured the task graph for **Entity Indexing (B1)** implementation (ISS-009) in the engram-ai-rust project.

## What Was Done

### 1. Updated `.gid/graph.yml`
- Added **7 new task nodes** for entity indexing implementation
- Added **13 new edges** defining task dependencies and relationships
- Preserved all existing Phase A tasks and dependencies
- Total graph now contains: **16 nodes**, **28 edges**

### 2. Created Documentation
- **`.gid/features/entity-indexing/task-graph.md`** - Visual task breakdown with execution order

### 3. Task Breakdown Structure

```
b1-entity-index (Parent)
├── entity-types (B1.1) - Foundation [80 LOC]
│   ├── entity-storage (B1.3) - CRUD [120 LOC]
│   │   ├── entity-add-raw (B1.4) - Write Path [80 LOC]
│   │   │   └── entity-backfill (B1.6) - CLI [40 LOC]
│   │   └── entity-recall (B1.5) - Read Path [150 LOC]
│   │       └── entity-backfill (B1.6) - CLI [40 LOC]
│   └── entity-unit-tests (B1.2) - Tests [100 LOC]
│       └── entity-integration-tests (B1.7) - E2E [150 LOC]
```

## Task Details

| Task ID | Title | LOC | Files | Status |
|---------|-------|-----|-------|--------|
| entity-types | Entity types & config structs | ~80 | entities.rs (new), config.rs, lib.rs, Cargo.toml | todo |
| entity-unit-tests | EntityExtractor unit tests | ~100 | entities.rs (tests) | todo |
| entity-storage | Storage entity CRUD methods | ~120 | storage.rs | todo |
| entity-add-raw | Integrate into add_raw() | ~80 | memory.rs | todo |
| entity-recall | Entity-aware recall channel | ~150 | entities.rs, memory.rs | todo |
| entity-backfill | Backfill CLI command | ~40 | main.rs | todo |
| entity-integration-tests | E2E tests | ~150 | tests/entity_integration_test.rs (new) | todo |
| **TOTAL** | | **~720** | | |

## Dependency Graph

```
Execution Order:
  1. entity-types (foundation)
  2. entity-storage + entity-unit-tests (parallel)
  3. entity-add-raw + entity-recall (parallel, both depend on storage)
  4. entity-backfill (depends on both add-raw and recall)
  5. entity-integration-tests (final validation)
```

### Critical Path
`entity-types` → `entity-storage` → `entity-add-raw` → `entity-backfill` → `entity-integration-tests`

Length: **5 tasks**

### Parallel Opportunities
- `entity-unit-tests` can run parallel to `entity-storage`
- `entity-add-raw` and `entity-recall` can run in parallel (both depend only on `entity-storage`)

## Files to Create/Modify

### New Files (2)
1. `src/entities.rs` (~380 lines total)
   - EntityType enum, ExtractedEntity struct, EntityExtractor
   - normalize_entity_name(), entity_recall()
   - Unit tests

2. `tests/entity_integration_test.rs` (~150 lines)
   - End-to-end entity indexing tests

### Modified Files (6)
1. `src/storage.rs` (+120 lines) - 8 new CRUD methods
2. `src/memory.rs` (+100 lines) - Write/read path integration
3. `src/main.rs` (+40 lines) - Backfill CLI command
4. `src/config.rs` (+20 lines) - EntityConfig struct
5. `src/lib.rs` (+2 lines) - Export entities module
6. `Cargo.toml` (+1 dep) - Add aho-corasick

## Dependencies

### New Crate Dependencies
```toml
aho-corasick = "1.1"  # For efficient multi-pattern entity extraction
```

### Task Dependencies
- **Phase prerequisite**: All B1 tasks depend on Phase A completion (a3-sql-cleanup)
- **Foundation**: All tasks depend directly or transitively on `entity-types`
- **Integration**: CLI and tests depend on core write/read path completion

## Validation

✅ All 7 entity tasks added to graph  
✅ All subtask_of edges created (7 edges to b1-entity-index)  
✅ All dependency edges created (6 inter-task dependencies)  
✅ Graph YAML is valid  
✅ No circular dependencies  
✅ Critical path identified  
✅ Parallel opportunities documented  

## Next Steps

To execute this plan:

1. **Start with entity-types** (B1.1)
   - No dependencies, must complete first
   - Creates foundational types and config

2. **Parallel development** (B1.2 + B1.3)
   - Run entity-unit-tests and entity-storage in parallel
   - Both only depend on entity-types

3. **Integration phase** (B1.4 + B1.5)
   - entity-add-raw and entity-recall can run in parallel
   - Both require entity-storage completion

4. **CLI tooling** (B1.6)
   - entity-backfill requires both write and read paths

5. **Final validation** (B1.7)
   - entity-integration-tests confirms entire system works

## Success Metrics

- [ ] All 7 tasks marked as `done`
- [ ] ~720 lines of new code written
- [ ] All tests passing (unit + integration)
- [ ] Entity extraction integrated into write path
- [ ] Entity-aware recall working as 4th hybrid channel
- [ ] Backfill command functional for existing memories
- [ ] No performance regression (<5ms overhead per operation)

## References

- **Design**: `.gid/features/entity-indexing/design.md`
- **Issue**: ISS-009
- **Graph**: `.gid/graph.yml`
- **Visual**: `.gid/features/entity-indexing/task-graph.md`
