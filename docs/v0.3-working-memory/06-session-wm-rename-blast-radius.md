# v0.3 Working Memory — Finding 06: SessionWorkingMemory Rename Blast Radius

> **Context**: DESIGN-v0.3 r1 review A1 proposed renaming `SessionWorkingMemory` to disambiguate from L2 `working_strength`. r1 estimated "~20 call sites to rename."
> **Date**: 2026-04-24
> **Method**: `.gid-v03-context/.gid/graph.db` (LSP-refined code graph) + ground-truth grep

## TL;DR

**Actual blast radius: 3 files, 22 symbol occurrences.** r1's "~20 call sites" was an overestimate — this rename is significantly cheaper than the review suggested, which strengthens the case for Option A (rename) over Option B (relabel L2) or Option C (just document containment).

## Ground Truth

```
grep -rn "SessionWorkingMemory" crates/engramai/src/ → 22 occurrences, 3 files
```

- **`src/session_wm.rs`** — definition + 5 unit tests
- **`src/lib.rs`** — public re-export
- **`src/memory.rs`** — 5 `confidence_tests` that exercise broadcast admission + Hebbian spreading through session working memory

**No production caller outside `session_wm.rs` itself.** `SessionRegistry.get_session()` is the sole entry point; all other cross-file usage is in test modules.

## Code Graph Validation

The LSP-refined graph in `.gid-v03-context/.gid/graph.db` agrees — with one major caveat.

### LSP-confident edges (confidence ≥ 0.9) — the real callers

| From | To | Conf |
|---|---|---|
| `func:src/memory.rs:confidence_tests::test_broadcast_admission_generates_confidence_signals` | `class:src/session_wm.rs:SessionWorkingMemory` | 1.0 |
| `func:src/memory.rs:confidence_tests::test_broadcast_admission_multiple_memories` | `class:src/session_wm.rs:SessionWorkingMemory` | 1.0 |
| `func:src/memory.rs:confidence_tests::test_broadcast_hebbian_spreading` | `class:src/session_wm.rs:SessionWorkingMemory` | 1.0 |
| `func:src/memory.rs:confidence_tests::test_broadcast_updates_hub_domain_state` | `class:src/session_wm.rs:SessionWorkingMemory` | 1.0 |
| `func:src/memory.rs:confidence_tests::test_broadcast_with_nonexistent_memory_is_safe` | `class:src/session_wm.rs:SessionWorkingMemory` | 1.0 |

Exactly 5 cross-file call edges, all test code in `memory.rs`. Matches grep ground truth.

### Low-confidence edges (0.6) — tree-sitter false positives

Many edges point at `SessionWorkingMemory.contains` at confidence=0.6 from wildly unrelated sites: `parse_soul`, `tokenize_cjk_boundaries`, `deserialize_flexible_string`, `find_aligned_drives`, `enforce_prompt_budget`, `hybrid_search`, etc.

**Root cause**: tree-sitter name-only matching (gid-rs ISS-002) — Rust's `.contains()` is a near-universal method (Vec, HashSet, HashMap, str, slices) and without LSP resolution, every `.contains()` call in the codebase gets speculatively wired to `SessionWorkingMemory.contains`. These edges carry `confidence: 0.6` as a deliberate marker.

**Takeaway**: When using the code graph as working memory, **always filter `confidence >= 0.9`** for call edges, otherwise you inherit hundreds of LSP-unresolved false positives. The graph metadata is self-reporting this uncertainty correctly; we just have to read it.

## Locally-defined symbols (all `defined_in` edges)

Defined by `session_wm.rs`:
- `struct SessionWorkingMemory` — the target
- `struct SessionRegistry` — entry-point registry
- `struct CachedScore` — value type stored in the buffer
- 19 methods on SessionWorkingMemory: `new`, `with_defaults`, `default`, `activate`, `activate_with_scores`, `get_score`, `set_query`, `last_query`, `prune`, `get_active_ids`, `len`, `is_empty`, `clear`, `contains`, `overlap`, `is_topic_continuous`, + tests
- 5 unit tests

Rename surface = struct name + all method doc/comment references within the file. `sed -i '' 's/SessionWorkingMemory/ActiveContext/g' session_wm.rs lib.rs memory.rs` handles the mechanical bulk; the tricky part is the struct name in doc comments referring to the concept.

## Implications for r1 Findings

**A1 (working-memory naming collision)**:
- r1 recommendation stands: Option A (rename `SessionWorkingMemory` → `ActiveContext` / `ConversationBuffer`) + Option C (document containment in §2).
- r1 cost estimate "~20 call sites" was inflated by ~6×. True cost: one sed pass across 3 files + manual review of ~5 test assertions + DESIGN §2 paragraph. **< 1 hour of work.**
- The low cost removes the last objection to Option A. Recommended decision: **proceed with rename.**

## Procedural Lesson

This is the textbook use case for the code graph as working memory:
1. Review surfaces an architectural claim ("~20 call sites").
2. Query `.gid-v03-context/.gid/graph.db` in 1ms for ground truth.
3. Validate against `grep` to catch LSP gaps (none here).
4. Correct the scope estimate before committing to a design decision.

**Without the code graph**: we'd have grepped (fine) but missed the structural pattern — that the only cross-file usage is test code, not production path. The graph's `calls` edges + confidence filtering gave us that structural insight in one query.

**Caveat surfaced**: LSP coverage is partial. Confidence field is load-bearing — must be filtered when querying call edges. Upgrading gid-rs LSP resolution (ISS-002, ISS-016) would remove the need for this filter.
