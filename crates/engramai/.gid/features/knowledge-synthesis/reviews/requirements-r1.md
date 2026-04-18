# Requirements Review: Knowledge Synthesis (ISS-005)

**Document**: `.gid/features/knowledge-synthesis/requirements.md`  
**Review Date**: 2026-04-09  
**Review Depth**: Quick (Phases 0, 1, 4 — Checks 0-6, 17-21)  
**Reviewer**: Claude Code (Requirements Document Reviewer skill)

---

## 🔴 Critical (blocks implementation)

### FINDING-1: [Check #6] GOAL-7: Implementation leakage — CLI command names
**GOAL-7** specifies exact CLI command names (`engram synthesize`, `engram synthesize --dry-run`, `engram insights`, `engram insight <id>`). This is a design decision, not a requirement. The Substitution Test fails: someone could satisfy "expose synthesis through CLI" with completely different command names (`engram create-insights`, `engram show-knowledge`, etc.) and still meet the actual requirement.

**Suggested fix**:
```markdown
### GOAL-7: CLI Access
The system must expose synthesis functionality through the CLI. Users must be able to:
- Trigger a synthesis cycle and store results
- Preview discovered clusters and candidate insights without making changes
- List all synthesized insights with their source counts
- Inspect a specific insight and view its source memories
```

### FINDING-2: [Check #6] GOAL-8: Implementation leakage — function signatures
**GOAL-8** specifies exact method names, struct names, and function signatures (`synthesize(&mut self) -> Result<SynthesisResult>`, `list_insights() -> Result<Vec<MemoryRecord>>`, etc.). These are design decisions. The requirement is "provide programmatic API access to synthesis" — the actual names and signatures belong in design/implementation.

**Suggested fix**:
```markdown
### GOAL-8: Programmatic API
The system must expose synthesis functionality programmatically (callable from RustClaw, OpenClaw, CLI, or any Rust code using the crate). The API must support:
- Running a synthesis cycle and returning results summary
- Querying all synthesized insights
- Querying the source memories for a given insight
- No dependency on CLI infrastructure — pure library interface
```

### FINDING-3: [Check #6] GOAL-9: Implementation leakage — configuration mechanism
**GOAL-9** specifies that parameters "must be configurable via `MemoryConfig`" — this is implementation detail (a specific struct name). The requirement is that parameters be configurable, not how/where they're stored.

**Suggested fix**:
```markdown
### GOAL-9: Configurable Synthesis Parameters
The system must allow configuration of synthesis behavior through the existing configuration mechanism:
- Minimum cluster size (default: 3 memories)
- Embedding similarity threshold for clustering (default: 0.75)
- Maximum memories per synthesis call (default: 10, to control LLM context size)
- Maximum insights per cycle (default: 5, to control LLM cost)
- Source demotion factor (default: 0.5 — multiply importance by this factor after synthesis)
```

### FINDING-4: [Check #6] GUARD-5: Implementation leakage — storage mechanism
**GUARD-5** specifies that insights "must be stored as regular `MemoryRecord` entries with `memory_type: factual`" — this is schema/implementation detail. The requirement is that insights participate in normal recall and be distinguishable from raw memories.

**Suggested fix**:
```markdown
### GUARD-5: Insight Identity
Synthesized insights must be stored in the same storage system as regular memories and must participate in normal recall operations. Insights must be distinguishable from raw memories when queried. The system must support filtering by insight vs. raw memory.
```

### FINDING-5: [Check #5] GOAL-2: Missing outcome specification
**GOAL-2** says "must produce an insight" and "must be more abstract than any individual source memory" but does not specify the expected outcome in measurable terms. How do we verify that the produced insight is actually "more abstract"? What happens if the LLM produces an insight that's just a verbatim copy of one source memory?

**Suggested fix**:
```markdown
### GOAL-2: Insight Generation
Given a cluster of related memories, the system must invoke the LLM to produce an insight — a new memory that captures the higher-order pattern, principle, or summary that the cluster collectively represents. The system must validate that the generated insight:
- (a) Is semantically distinct from any single source memory (embedding similarity to any single source < 0.95)
- (b) Has content length ≥ the median of source memory content lengths (prevents degenerate truncation)
- (c) Successfully stores as a new memory with synthesis metadata
If validation fails, the synthesis attempt for that cluster must be skipped and logged.
```

### FINDING-6: [Check #5] GOAL-5: Missing trigger specification
**GOAL-5** specifies expected behavior (idempotency, no duplicates) but does not specify the trigger: "An insight that was already synthesized should not be re-synthesized unless its source cluster has grown (new memories added to the cluster since last synthesis)." What defines "cluster has grown"? How many new memories? What if one new memory is added — does that trigger re-synthesis?

**Suggested fix**:
```markdown
### GOAL-5: Idempotent Synthesis
Running synthesis on the same cluster must not produce duplicate insights. The system must detect when an existing insight already covers a cluster (by comparing cluster membership) and skip re-synthesis. Re-synthesis is triggered only when:
- (a) The cluster has grown by ≥ 50% in size (e.g., 3-memory cluster must reach ≥5 memories), OR
- (b) The existing insight is older than 30 days AND the cluster has ≥1 new memory
If re-synthesis is triggered, the old insight must be superseded (marked as deprecated/archived) and the new insight must reference both the old insight and the expanded source set.
```

### FINDING-7: [Check #17] GOAL-6: Technology assumptions without justification
**GOAL-6** requires LLM capability and says "must use the existing extractor interface (or a similar trait)". This assumes an extractor interface exists and is appropriate for synthesis. Is this interface documented? Does it support the prompt patterns needed for synthesis vs. extraction? What if it doesn't support multi-shot prompts or requires schema that doesn't fit insights?

**Suggested fix**: Add to constraints or non-functional requirements:
```markdown
### GUARD-6: LLM Interface Compatibility
Synthesis must use the existing LLM abstraction layer (extractor trait or equivalent) without modification. If the existing interface is insufficient for synthesis prompts, synthesis must be deferred until the interface is extended. The synthesis prompt must fit within the existing interface's capabilities (single-turn completion, no streaming, no function calling).
```

---

## 🟡 Important (should fix before implementation)

### FINDING-8: [Check #1] GOAL-1: Vague — "semantically related"
**GOAL-1** uses "semantically related memories" without defining semantic relatedness beyond the numeric thresholds. The thresholds are concrete, but the term itself is vague (could mean topic similarity, temporal proximity, causal connection, etc.).

**Suggested fix**: Rephrase to:
```markdown
### GOAL-1: Cluster Discovery
The system must identify groups of memories that meet cluster criteria. A cluster is a set of ≥3 memories meeting ANY of:
- Pairwise embedding cosine similarity exceeds a configurable threshold (default 0.75)
- Share ≥2 entity references (same entity UUID appears in ≥3 memories)
- Connected by Hebbian links with strength ≥ 0.3

Each memory may belong to multiple clusters. Clusters may overlap.
```

### FINDING-9: [Check #2] GOAL-6: Not fully testable — "function without LLM"
**GOAL-6** says "The system must function without LLM (clustering still works, synthesis is skipped with a clear error/log)". What does "function" mean? Does it return an error? Does it return success with zero insights? Does it log a warning or an error? This needs a clear pass/fail condition.

**Suggested fix**:
```markdown
### GOAL-6: LLM-Based Synthesis
[... keep existing content ...]

When no LLM provider is configured or available:
- Cluster discovery must complete successfully
- Synthesis phase must be skipped
- The synthesis result must indicate success with zero insights generated and a status code indicating "LLM unavailable"
- A warning-level log message must be emitted: "Synthesis skipped: no LLM provider configured"
```

### FINDING-10: [Check #18] GOAL-6: External dependency not versioned
**GOAL-6** says "must use the existing extractor interface" but doesn't specify what version or state of that interface. If the interface is still under development, which version is required? Does it need to be in `main` branch? Tagged release?

**Suggested fix**: Add to constraints:
```markdown
### GUARD-7: Extractor Interface Stability
Synthesis depends on the extractor trait being stable (merged to main, interface frozen). If the extractor trait is still in flux, synthesis implementation must be blocked until the interface is stabilized. The trait must support at minimum: (a) arbitrary prompt text, (b) synchronous completion, (c) error handling for LLM unavailability.
```

### FINDING-11: [Check #19] GOAL-2: Missing data format specification
**GOAL-2** says insights must be "stored" but doesn't specify what data accompanies an insight. Does an insight have:
- Original source content embedded/copied?
- Timestamp of synthesis?
- Embedding vector computed?
- Entities extracted?
All these affect storage size and query performance.

**Suggested fix**: Add a new GOAL:
```markdown
### GOAL-11: Insight Metadata
Each synthesized insight must include:
- The LLM-generated insight text (stored as memory content)
- Timestamp of synthesis (when the insight was created)
- List of source memory IDs (for provenance, see GOAL-3)
- Embedding vector computed from the insight text (enables similarity search)
- Entity extraction applied to insight text (enables entity-based queries)

The total storage overhead per insight must not exceed 2x the median source memory size.
```

### FINDING-12: [Check #20] GOAL-4: No migration/compatibility plan
**GOAL-4** introduces "demotion" (reducing importance, moving to archive layer). For existing databases with memories that were never part of synthesis, how do you distinguish "never synthesized" from "synthesized and demoted"? Is there a migration needed to mark all existing memories as "not yet considered for synthesis"?

**Suggested fix**: Add to constraints:
```markdown
### GUARD-8: Synthesis State Migration
For databases created before synthesis feature exists, all memories must default to "never synthesized" state. No migration/schema change is required — absence of synthesis metadata implies the memory has not been part of any synthesis. This enables incremental adoption (users can add synthesis to existing databases without rebuilding).
```

---

## 🟢 Minor (can fix during implementation)

### FINDING-13: [Check #1] Success Criteria: Vague — "rank higher"
Success criteria says "Insights rank higher than individual source memories in recall for the same query" but doesn't define "rank higher" quantitatively. Is this measured by effective_strength? By recall score? By position in result list?

**Suggested fix**: Clarify success criteria:
```markdown
- Insights rank higher than individual source memories in recall for the same query: For a query that matches a cluster topic, the synthesized insight must appear in the top 3 recall results, while ≥50% of source memories must rank below position 10.
```

### FINDING-14: [Check #4] GOAL-1: Compound requirement
**GOAL-1** defines three different clustering mechanisms (embedding similarity OR entity sharing OR Hebbian links) in one requirement. These could be split into atomic requirements for independent testing.

**Suggested fix**: Split into GOAL-1a, GOAL-1b, GOAL-1c:
```markdown
### GOAL-1a: Embedding-Based Clustering
The system must identify clusters where ≥3 memories have pairwise embedding cosine similarity exceeding a configurable threshold (default 0.75).

### GOAL-1b: Entity-Based Clustering
The system must identify clusters where ≥3 memories share ≥2 entity references (same entity UUID).

### GOAL-1c: Hebbian-Based Clustering
The system must identify clusters where ≥3 memories are connected by Hebbian links with strength ≥ 0.3.
```

---

## ✅ Passed Checks

### Phase 0: Document Size
- ✅ **Check #0**: Document has 10 GOALs, 5 GUARDs — well within the 15-GOAL limit. Appropriate size for a single feature.

### Phase 1: Individual Requirement Quality
- ✅ **Check #2**: Testability — 8/10 GOALs have clear pass/fail conditions (GOAL-2 and GOAL-6 flagged above)
- ✅ **Check #3**: Measurability — Quantitative requirements have concrete numbers:
  - GOAL-1: 0.75 threshold, ≥3 memories, ≥2 entities, ≥0.3 Hebbian strength
  - GOAL-4: Source demotion (though mechanism not quantified in requirement)
  - GOAL-9: All defaults specified numerically
  - Success criteria: 100+ memories, ≥3 insights, <30s completion
- ✅ **Check #4**: Atomicity — 9/10 GOALs describe one thing (GOAL-1 compound but related)
- ✅ **Check #5**: Completeness — 8/10 GOALs specify actor/behavior/outcome (GOAL-2, GOAL-5 flagged above)

### Phase 4: Implementability
- ✅ **Check #21**: Scope boundaries — Excellent non-goals section (4 NON-GOALs explicitly stated):
  - Real-time synthesis
  - Multi-level synthesis
  - Automatic scheduling
  - Cross-namespace synthesis

---

## 📊 Summary

| Metric | Count |
|---|---|
| **Total GOALs** | 10 |
| **Total GUARDs** | 5 |
| **Critical findings** | 7 |
| **Important findings** | 5 |
| **Minor findings** | 2 |
| **Checks passed** | 7/12 (quick review) |

### Breakdown by Check

| Check # | Check Name | Status | Notes |
|---|---|---|---|
| 0 | Document size | ✅ Pass | 10 GOALs (≤15 limit) |
| 1 | Specificity | 🟡 Partial | GOAL-1 "semantically related" vague; Success criteria "rank higher" vague |
| 2 | Testability | 🟡 Partial | GOAL-6 "function without LLM" ambiguous |
| 3 | Measurability | ✅ Pass | All numeric requirements have concrete values |
| 4 | Atomicity | 🟢 Minor | GOAL-1 compound but acceptable |
| 5 | Completeness | 🔴 Critical | GOAL-2 missing outcome validation; GOAL-5 missing trigger spec |
| 6 | Implementation leakage | 🔴 **CRITICAL** | GOAL-7, 8, 9, GUARD-5 all leak design decisions |
| 17 | Technology assumptions | 🔴 Critical | GOAL-6 assumes extractor interface exists/is suitable |
| 18 | External dependencies | 🟡 Important | Extractor interface not versioned |
| 19 | Data requirements | 🟡 Important | GOAL-2 missing insight data format spec |
| 20 | Migration/compatibility | 🟡 Important | No migration plan for existing databases |
| 21 | Scope boundaries | ✅ Pass | Excellent non-goals section |

---

## 🎯 Primary Issue: Implementation Leakage (Check #6)

**This is the core blocker.** 4/10 GOALs (40%) and 1/5 GUARDs specify implementation details instead of requirements:

- **GOAL-7**: CLI command names (`engram synthesize`, `engram insights`)
- **GOAL-8**: Struct names, method signatures (`Memory`, `synthesize(&mut self)`)
- **GOAL-9**: Config struct name (`MemoryConfig`)
- **GUARD-5**: Storage schema details (`MemoryRecord`, `memory_type: factual`)

### Why This Matters

These requirements will pass all other quality checks but cause endless review cycles because **the contradictions are structural**. For example:
- GOAL-8 says insights must be `MemoryRecord` entries
- But what if during design you discover that insights need different fields than regular memories?
- The requirement forces a design decision before design phase even starts
- Alternative implementations (separate `InsightRecord` type, insight-specific metadata table) are ruled out by fiat, not by actual requirements

### The Substitution Test

For each flagged GOAL, ask: "Could someone satisfy this with a completely different internal implementation?"

- **GOAL-7 current**: No — command names are hardcoded
- **GOAL-7 fixed**: Yes — any CLI interface that exposes the four capabilities works
- **GOAL-8 current**: No — method names and signatures are hardcoded
- **GOAL-8 fixed**: Yes — any programmatic API works (trait, struct methods, FFI, whatever)

---

## 🔧 Recommendation

**Status**: ⚠️ **Needs fixes before design phase**

### Must Fix (Critical)
1. Apply FINDING-1 through FINDING-7 — remove all implementation leakage
2. These are the root cause of requirements that "look good" but constrain implementation prematurely

### Should Fix (Important)
3. Apply FINDING-8 through FINDING-12 — clarify ambiguities and add missing specifications
4. These will prevent questions during implementation

### Can Defer (Minor)
5. FINDING-13, FINDING-14 are editorial improvements — fix during design if convenient

### Implementation Clarity
**Medium-Low** — The feature concept is clear and well-motivated. The technical infrastructure (embeddings, entities, Hebbian links) exists. The problem is that ~40% of the requirements specify HOW instead of WHAT, which will either:
- Force unnecessary constraints on the implementation, OR
- Require re-writing requirements during design when conflicts emerge

Fixing the implementation leakage will raise clarity to **High**.

---

## 📋 Next Steps

1. **Human review**: Approve/reject findings
2. **Apply fixes**: Run `apply-requirements-review` skill to update the requirements doc
3. **Re-review** (optional): After fixes, run a standard or full review to check coverage/consistency (Phases 2-3)
4. **Proceed to design**: Once critical findings are resolved, move to ISS-005 design phase
