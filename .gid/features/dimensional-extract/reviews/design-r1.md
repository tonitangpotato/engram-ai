# Design Review: Dimensional Memory Extraction — R1

**Document**: `.gid/features/dimensional-extract/design.md`
**Requirements**: `.gid/features/dimensional-extract/requirements.md`
**Date**: 2026-04-21
**Depth**: Standard (Phase 0-5)

---

## Summary

Found **9 findings**: 2 Critical, 3 Important, 4 Minor. **All applied.**

---

## Findings

### FINDING-1 ✅ Applied
- **Was**: ExtractedFact dropped 3 existing fields (confidence, valence, domain) that drive interoceptive emotion system
- **Change**: Added `confidence`, `valence`, `domain` to ExtractedFact struct + structured output schema. Updated change note.

### FINDING-2 ✅ Applied
- **Was**: Default TypeWeights self-contradictory (§3.3 said 0.5, §3.5 corrected to 1.0)
- **Change**: Default::default() set to all 1.0 from the start. Removed contradictory "修正" paragraphs. Single source of truth.

### FINDING-3 ✅ Applied
- **Was**: max aggregation penalizes new memories in neutral query (0.5 vs 1.0)
- **Change**: Replaced sum/7 approach with max strategy throughout. Added explicit analysis of neutral query trade-off as acceptable precision-recall trade-off.

### FINDING-4 ✅ Applied
- **Was**: §2 Architecture still said "9 维度"
- **Change**: Updated to "11 维度"

### FINDING-5 ✅ Applied
- **Was**: §3.1 said "8 个 Option 字段"
- **Change**: Updated to "10 个 Option<String> 维度字段"

### FINDING-6 ✅ Applied
- **Was**: source_text only stored on first fact, dedup could lose it
- **Change**: source_text now attached to every extracted fact

### FINDING-7 ✅ Applied
- **Was**: Fallback prompt behavior undefined
- **Change**: Explicitly stated fallback uses old prompt, legacy parser path, dimensions empty, type_weights default

### FINDING-8 ✅ Applied
- **Was**: memory_type parameter's new role undocumented
- **Change**: Documented that memory_type param is ignored when extract succeeds, only used on fallback

### FINDING-9 ✅ Applied
- **Was**: Implementation plan listed add_raw as main change point
- **Change**: Clarified add_to_namespace() is the main change point. Removed incorrect types.rs item.

---

## Summary
- Applied: 9/9
- Skipped: 0
