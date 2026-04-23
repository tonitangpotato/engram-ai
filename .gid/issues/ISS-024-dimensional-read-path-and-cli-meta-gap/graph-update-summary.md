# ISS-024 Graph Update Summary

**Date:** 2024
**Status:** Complete ✓

## Overview

Added ISS-024 tracking nodes to `.gid/graph.yml` to manage the dimensional metadata pipeline bugfix work. This issue addresses two critical gaps:

- **Gap A (write):** `engram store` CLI missing `--meta` flag → caller-supplied dimensions silently dropped
- **Gap B (read):** Retrieval path completely ignores `metadata.dimensions.*` → no ranking benefit

## Nodes Added

### Parent Task
- **iss-024** - Parent tracking node for the entire issue
  - Status: `blocked` (waiting on design)
  - Priority: 45 (high — blocks ISS-020 Phase B)
  - Scope: Changes 1-4 as defined in investigation.md
  - Out of scope: Change 3b (8th channel), ISS-022 schema refactor, backfill

### Design Phase
- **iss-024-design** - Design phase addressing 5 underspecified areas
  - Status: `todo`
  - Deliverable: design-1-*.md
  - Must address:
    1. Time parsing library/rules for Change 3a
    2. Complete adapter inventory for Change 2
    3. Reserved namespace semantics (exact error messages)
    4. Contract test framework (reusable pattern)
    5. Interface shape for 3a → 3b migration path

### Implementation Tasks (all blocked by design)
- **iss-024-cli-meta** - Change 1: Add `--meta` flag to CLI
  - ~40 lines in src/main.rs
  - Implements `parse_kv` helper, reserved key validation
  - Closes Gap A

- **iss-024-adapter-rc** - Change 2: RC checks at adapter call sites
  - Variable LOC across adapters
  - LoCoMo bench runner, RustClaw skill, others TBD in design
  - Defense in depth (equal priority to Change 1, not just nice-to-have)

- **iss-024-temporal-dim** - Change 3a: Temporal scoring from dimensions
  - ~80 lines in src/memory.rs
  - Natural-language time parsing (library TBD in design)
  - max(insertion_time_score, dimension_time_score)
  - Closes Gap B (surgical approach; full 8th channel deferred to ISS-020)

- **iss-024-contract-test** - Change 4: End-to-end CLI boundary test
  - ~150 lines in tests/dimensional_contract_test.rs
  - **The contract:** CLI → store → recall → ranking uses dimensions
  - Reusable test harness for future CLI integration work
  - Depends on Changes 1-3 all landing

## Dependency Graph

```
iss-024 (parent)
  ├─ iss-024-design (todo) ──┐
  ├─ iss-024-cli-meta ───────┼─── blocked by design
  ├─ iss-024-adapter-rc ─────┼─── blocked by design
  ├─ iss-024-temporal-dim ───┼─── blocked by design
  └─ iss-024-contract-test ──┴─── blocked by design
       │
       ├─ depends on iss-024-cli-meta
       ├─ depends on iss-024-adapter-rc
       └─ depends on iss-024-temporal-dim

iss-020 (LoCoMo dim-aware ranking)
  └─ depends on iss-024-contract-test
```

**Key insight:** ISS-020 Phase B (dimension_match 8th channel) is blocked until this entire pipeline is proven end-to-end. The contract test is the gate.

## Edges Added

### Subtask relationships
- iss-024-design → iss-024
- iss-024-cli-meta → iss-024
- iss-024-adapter-rc → iss-024
- iss-024-temporal-dim → iss-024
- iss-024-contract-test → iss-024

### Dependency chain
- iss-024-cli-meta → iss-024-design (blocks)
- iss-024-adapter-rc → iss-024-design (blocks)
- iss-024-temporal-dim → iss-024-design (blocks)
- iss-024-contract-test → iss-024-design (blocks)
- iss-024-contract-test → iss-024-cli-meta (depends)
- iss-024-contract-test → iss-024-adapter-rc (depends)
- iss-024-contract-test → iss-024-temporal-dim (depends)

### External blocker
- iss-020 → iss-024-contract-test (depends)
  - ISS-020 Phase B cannot proceed until dimensional pipeline is proven live

## Priority Rationale

All ISS-024 nodes set to **priority: 45**, placed between:
- ISS-021 (priority 40) — sub-dimension coverage audit
- ISS-020 (priority 50) — LoCoMo dim-aware retrieval

**Reasoning:**
- Higher than ISS-021 because even with 100% dimension coverage, the read path is broken (Gap B)
- Lower than ISS-020 because ISS-020 *needs* this fix to deliver value
- Marked as blocker for ISS-020 Phase B to make the dependency explicit

## Next Steps

1. **Design phase** (iss-024-design):
   - Answer the 5 questions in investigation.md §Design phase MUST address
   - Create design-1-*.md in this folder
   - Complete adapter inventory (grep ecosystem)
   - Select time parsing library + API design
   - Define exact reserved-key error messages
   - Design reusable contract test pattern

2. **After design approval:**
   - Unblock implementation tasks (flip status from `blocked` → `todo`)
   - Changes 1-3 can proceed in parallel
   - Change 4 (contract test) requires 1-3 to land first

3. **After contract test passes:**
   - Unblock ISS-020 Phase B
   - LoCoMo can run ablation studies with real dimension-aware ranking

## Validation

✓ Graph file is valid YAML (validated with Ruby YAML parser)
✓ All node IDs unique and follow iss-024-* pattern
✓ All edges reference existing nodes
✓ Dependency chain is acyclic
✓ Priority ordering aligns with blocker relationships

## File Modifications

- `.gid/graph.yml` - Added 6 nodes + 14 edges (lines appended to end of file)
- No existing nodes modified (append-only change)
