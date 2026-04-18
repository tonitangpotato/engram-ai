# Entity Indexing (B1) - Execution Checklist

**Status**: Ready for implementation  
**Issue**: ISS-009  
**Total Tasks**: 7  
**Estimated LOC**: ~720 lines  

---

## 🎯 Execution Order

### Phase 1: Foundation (Day 1)
- [ ] **B1.1: entity-types** (~2 hours)
  - [ ] Create `src/entities.rs` with EntityType enum
  - [ ] Add ExtractedEntity, EntityRecord structs
  - [ ] Implement normalize_entity_name()
  - [ ] Add EntityConfig to `src/config.rs`
  - [ ] Add aho-corasick to Cargo.toml
  - [ ] Export module in lib.rs
  - [ ] Verify compilation

### Phase 2: Parallel Development (Day 1-2)
- [ ] **B1.2: entity-unit-tests** (~1.5 hours) [PARALLEL]
  - [ ] Test known entity extraction (Aho-Corasick)
  - [ ] Test regex patterns (ISS-XXX, URLs, files)
  - [ ] Test normalization edge cases
  - [ ] Test dedup within content
  - [ ] All tests passing

- [ ] **B1.3: entity-storage** (~3 hours) [PARALLEL]
  - [ ] Implement upsert_entity() with deterministic IDs
  - [ ] Implement link_memory_entity()
  - [ ] Implement upsert_entity_relation()
  - [ ] Implement find_entities()
  - [ ] Implement get_entity_memories()
  - [ ] Implement get_related_entities()
  - [ ] Implement get_entity(), count_entities()
  - [ ] Add unit tests for each method

### Phase 3: Integration (Day 2-3)
- [ ] **B1.4: entity-add-raw** (~2 hours) [PARALLEL]
  - [ ] Initialize EntityExtractor in MemorySystem::new()
  - [ ] Hook entity extraction into add_raw()
  - [ ] Extract entities from content
  - [ ] Upsert entities to DB
  - [ ] Link memory to entities
  - [ ] Build co-occurrence relations (cap at 10)
  - [ ] Test write path works

- [ ] **B1.5: entity-recall** (~3 hours) [PARALLEL]
  - [ ] Implement entity_recall() in entities.rs
  - [ ] Extract entities from query
  - [ ] Find matching entities in DB
  - [ ] Get memory IDs for entities + 1-hop relations
  - [ ] Score memories by entity relevance
  - [ ] Integrate into recall_from_namespace()
  - [ ] Add entity channel weight to hybrid search
  - [ ] Test recall path works

### Phase 4: Tooling & Validation (Day 3-4)
- [ ] **B1.6: entity-backfill** (~1.5 hours)
  - [ ] Add backfill_entities() function
  - [ ] Add CLI subcommand: engram backfill-entities
  - [ ] Support --namespace filter
  - [ ] Add progress logging
  - [ ] Add stats output
  - [ ] Test on sample data
  - [ ] Test idempotency

- [ ] **B1.7: entity-integration-tests** (~2 hours)
  - [ ] Test entity extraction + storage in add_raw()
  - [ ] Test entity-aware recall finds correct memories
  - [ ] Test co-occurrence relations
  - [ ] Test backfill on existing memories
  - [ ] Test entity dedup (normalized names)
  - [ ] Test multi-entity recall boost
  - [ ] Test namespace isolation
  - [ ] All integration tests passing

---

## 📋 Pre-Flight Checklist

Before starting:
- [ ] Phase A tasks completed (a3-sql-cleanup done)
- [ ] Database backed up
- [ ] Current main branch tests passing
- [ ] Design document reviewed (.gid/features/entity-indexing/design.md)
- [ ] Development environment ready

---

## 🧪 Testing Strategy

### Unit Tests (Per Task)
- Each task should add its own unit tests
- Run `cargo test` after each task completion
- Aim for >80% code coverage on new code

### Integration Tests (Final)
- Run full test suite: `cargo test --all`
- Test on real-world data (RustClaw memories)
- Performance benchmark: <5ms overhead per operation

### Manual Verification
```bash
# After B1.4 (write path)
engram add "Working on ISS-009 entity indexing" --namespace test
sqlite3 engram.db "SELECT * FROM entities WHERE namespace='test'"

# After B1.5 (read path)  
engram recall "entity indexing" --namespace test
# Should show entity-matched results

# After B1.6 (backfill)
engram backfill-entities --namespace rustclaw
# Should process existing memories and extract entities
```

---

## 📊 Progress Tracking

### Completion Criteria
- [ ] All 7 tasks marked as `done` in graph.yml
- [ ] ~720 lines of code written
- [ ] All unit tests passing (>80% coverage)
- [ ] All integration tests passing
- [ ] Performance benchmarks met (<5ms overhead)
- [ ] Documentation updated
- [ ] Code reviewed
- [ ] Merged to main branch

### Blockers / Issues
_Track any blockers encountered during implementation_

| Task | Issue | Resolution | Status |
|------|-------|------------|--------|
| - | - | - | - |

---

## 🚀 Deployment

### Pre-Deploy Checks
- [ ] All tests passing on CI
- [ ] Performance benchmarks met
- [ ] Database migrations tested
- [ ] Rollback plan documented

### Deploy Steps
1. [ ] Merge feature branch to main
2. [ ] Tag release: `git tag v0.3.0-entity-indexing`
3. [ ] Deploy to staging
4. [ ] Run backfill on staging data
5. [ ] Verify entity recall working
6. [ ] Deploy to production
7. [ ] Monitor performance metrics

### Post-Deploy Validation
- [ ] Entity extraction working on new memories
- [ ] Entity recall improving search quality
- [ ] No performance degradation
- [ ] Backfill completed on production data
- [ ] Entity statistics look reasonable

---

## 📝 Notes

### Key Design Decisions
- Deterministic entity IDs (SHA256 of normalized name)
- Aho-Corasick for efficient known-entity extraction
- Co-occurrence relations capped at 10 entities to avoid O(n²)
- Entity recall weight: 0.15 (tunable)
- 1-hop relation traversal for related entities

### Performance Targets
- Entity extraction: <3ms per memory
- Entity recall: <20ms per query
- Backfill: >100 memories/second

### Future Enhancements (Out of Scope)
- Multi-hop entity relations (2+ hops)
- Entity type-specific recall weights
- Fuzzy entity matching (typo tolerance)
- Entity popularity scoring
- Cross-namespace entity linking

---

**Last Updated**: 2024-04-09  
**Owner**: Claude (ritual executor)  
**Reviewer**: TBD  
