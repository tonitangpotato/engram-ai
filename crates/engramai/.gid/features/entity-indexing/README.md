# Entity Indexing (B1) - Implementation Plan

**Feature**: Entity Indexing  
**Issue**: ISS-009  
**Status**: ✅ Task graph configured, ready for implementation  
**Priority**: 20 (Phase B - Core improvements)  

---

## 📖 Quick Links

| Document | Purpose |
|----------|---------|
| [design.md](./design.md) | Original feature design and requirements |
| [task-graph.md](./task-graph.md) | Visual task breakdown with diagrams |
| [TASK_GRAPH_COMPLETE.md](./TASK_GRAPH_COMPLETE.md) | Comprehensive summary and metrics |
| [EXECUTION_CHECKLIST.md](./EXECUTION_CHECKLIST.md) | Phase-by-phase execution guide |

---

## 🎯 What Is This?

Entity Indexing (B1) populates the empty `entities`, `memory_entities`, and `entity_relations` tables in the engramai database. This enables **concept-level jumps** during recall instead of relying purely on vector search.

**Key Benefits**:
- Extract entities (people, projects, technologies, concepts) from memories
- Enable entity-aware recall as 4th hybrid search channel
- Build knowledge graph via entity co-occurrence relations
- Backfill existing memories with entity extraction

---

## 📊 Implementation Overview

### Task Breakdown: 7 Tasks

```
b1-entity-index (Parent)
├── B1.1: entity-types           (~80 LOC)   - Foundation
├── B1.2: entity-unit-tests      (~100 LOC)  - Tests [PARALLEL]
├── B1.3: entity-storage         (~120 LOC)  - CRUD [PARALLEL]
├── B1.4: entity-add-raw         (~80 LOC)   - Write path
├── B1.5: entity-recall          (~150 LOC)  - Read path
├── B1.6: entity-backfill        (~40 LOC)   - CLI command
└── B1.7: entity-integration     (~150 LOC)  - E2E tests
```

**Total**: ~720 lines of code

### Dependency Chain

```
entity-types (foundation)
  ├─> entity-storage + entity-unit-tests (parallel)
  │     ├─> entity-add-raw + entity-recall (parallel)
  │     │     └─> entity-backfill
  │     │           └─> entity-integration-tests
```

**Critical Path**: 5 tasks  
**Parallel Opportunities**: 2

---

## 📁 Files Created/Modified

### New Files (2)
- `src/entities.rs` (~380 lines)
  - EntityType enum, ExtractedEntity struct
  - EntityExtractor with Aho-Corasick
  - entity_recall() function
  - Unit tests

- `tests/entity_integration_test.rs` (~150 lines)
  - End-to-end entity indexing tests

### Modified Files (6)
- `src/storage.rs` (+120 lines) - 8 CRUD methods
- `src/memory.rs` (+100 lines) - Write/read path integration
- `src/main.rs` (+40 lines) - Backfill CLI command
- `src/config.rs` (+20 lines) - EntityConfig
- `src/lib.rs` (+2 lines) - Module export
- `Cargo.toml` (+1 dep) - aho-corasick

---

## 🚀 Getting Started

### 1. Review the Design
Read [design.md](./design.md) to understand:
- Entity types (8 types: Person, Project, Technology, etc.)
- Extraction strategy (Aho-Corasick + regex patterns)
- Storage schema (3 tables)
- Recall integration (4th hybrid channel)

### 2. Check the Task Graph
Review [task-graph.md](./task-graph.md) for:
- Visual dependency diagram
- Execution order
- LOC estimates per task

### 3. Use the Execution Checklist
Follow [EXECUTION_CHECKLIST.md](./EXECUTION_CHECKLIST.md):
- Phase-by-phase implementation guide
- Testing strategy per task
- Pre-flight and post-deploy checklists

### 4. Start Implementation
Begin with **B1.1: entity-types** (foundational task):
```bash
# Create the foundational types
# See EXECUTION_CHECKLIST.md Phase 1 for details
```

---

## ✅ Success Criteria

- [ ] All 7 tasks completed and marked as `done` in `.gid/graph.yml`
- [ ] ~720 lines of new code written
- [ ] All unit tests passing (>80% coverage)
- [ ] All integration tests passing
- [ ] Entity extraction working on `add_raw()`
- [ ] Entity-aware recall integrated as 4th channel
- [ ] Backfill command functional
- [ ] Performance: <5ms overhead per operation
- [ ] Documentation updated

---

## 🧪 Testing Strategy

### Unit Tests (Per Task)
Each task adds its own unit tests:
- Test entity extraction patterns
- Test CRUD operations
- Test normalization logic

### Integration Tests (Final)
End-to-end validation:
- Entity extraction + storage working
- Entity-aware recall finding correct memories
- Co-occurrence relations built properly
- Backfill processing existing memories

### Manual Verification
```bash
# Test write path
engram add "Working on ISS-009" --namespace test

# Test read path  
engram recall "entity indexing" --namespace test

# Test backfill
engram backfill-entities --namespace rustclaw
```

---

## 📈 Estimated Timeline

| Phase | Tasks | Time | Status |
|-------|-------|------|--------|
| **Phase 1: Foundation** | B1.1 | ~2 hours | ⏳ todo |
| **Phase 2: Parallel Dev** | B1.2, B1.3 | ~4.5 hours | ⏳ todo |
| **Phase 3: Integration** | B1.4, B1.5 | ~5 hours | ⏳ todo |
| **Phase 4: Tooling** | B1.6, B1.7 | ~3.5 hours | ⏳ todo |
| **Total** | 7 tasks | **~15 hours** | |

Estimated completion: **3-4 working days** (with testing/reviews)

---

## 🔗 Related Issues

- **ISS-009**: Entity indexing implementation (this feature)
- **ISS-003**: Memory deduplication (B2, depends on B1)
- **ISS-007**: Confidence scoring (B3, depends on B1)
- **ISS-002**: Recency tuning (B4, depends on B3)

---

## 📚 Technical Details

### Key Technologies
- **aho-corasick**: Efficient multi-pattern string matching for known entities
- **SQLite**: Entity storage in existing tables (entities, memory_entities, entity_relations)
- **SHA256**: Deterministic entity ID generation (hash of normalized name)

### Entity Types Supported
1. Person (@mentions, known names)
2. Project (known project names)
3. Technology (Rust, Python, etc.)
4. Concept (memory, recall, activation, etc.)
5. File (file paths)
6. Url (HTTP/HTTPS URLs)
7. Organization (company names)
8. Other (catch-all)

### Performance Targets
- Entity extraction: <3ms per memory
- Entity recall: <20ms per query
- Backfill: >100 memories/second

---

## 🎉 Ready to Start!

All task graph configuration is complete. Follow the execution checklist to implement Entity Indexing (B1) for engramai.

**Next Step**: Review [EXECUTION_CHECKLIST.md](./EXECUTION_CHECKLIST.md) and start with Phase 1 (entity-types).

---

_Last Updated: 2024-04-09_  
_Task Graph Configured By: Claude (ritual phase executor)_  
