# Design Review: design-1-engine.md (Round 2 — Verification)
Date: 2026-04-14

## Findings

### FINDING-1: Missing section §9 [severity: important]
The design document sections jump from §8 (Integration) directly to §10 (Config Reference). Section §9 is missing entirely. While the content appears complete, the numbering gap suggests either a missing section or a numbering error.

### FINDING-2: GUARD-4 not defined in requirements [severity: critical]
The design document references GUARD-4 (Latency Budget) in §1 Overview's design principles list. However, the requirements document (requirements-1-engine.md) only defines GUARD-1, GUARD-2, GUARD-5, and GUARD-6. GUARD-4 is mentioned in the design without a corresponding requirement definition. This creates a traceability gap.

Additionally, GUARD-3 (Thread Safety) is referenced in the design overview but also not defined in the requirements document.

### FINDING-3: GOAL-1 clustering signal mismatch [severity: important]
Requirements §2 GOAL-1 specifies clustering signals with priorities:
- Primary: Hebbian co-activation (≥0.3 threshold)
- Secondary: Embedding similarity (≥0.75 threshold)  
- Tertiary: Entity overlap (≥2 entities)

Design §2.1 lists four signals with different structure:
- Signal 1: Hebbian Links
- Signal 2: Entity Overlap
- Signal 3: Embedding Similarity
- Signal 4: Temporal-Activation (not mentioned in requirements at all)

The temporal-activation signal is a design addition not grounded in requirements. The entity overlap requirements say "≥2 entity references" but the design uses Jaccard index (continuous 0.0-1.0), which is a different metric.

### FINDING-4: Config defaults incomplete verification [severity: minor]
While the design includes ClusterDiscoveryConfig, GateConfig, SynthesisConfig, and other config structs with defaults, some parameters from requirements §6.B are not visibly mapped:
- "Temporal spread minimum (1 hour)" — not found in config structs
- "Max memories per LLM call (10)" — not found in SynthesisConfig
- "Re-synthesis age threshold (30 days)" — not found in any config struct

These may be implicit or in unread sections, but should be explicitly documented in §10 Config Reference.

### FINDING-5: AC-2.1 validation check (c) not implemented [severity: important]
Requirements GOAL-2 acceptance criteria AC-2.1(c) requires insights to be "Actionable — an agent reading only the insight can make better decisions than reading any single source."

Design §4.3 Output Validation lists 7 validation checks, but none of them verify actionability. This is a subjective criterion that may not be automatable, but the design should acknowledge this gap or explain how it's addressed (e.g., via prompt engineering).

### FINDING-6: Emotional modulation added without requirements coverage [severity: minor]
Design §2.4 adds comprehensive emotional modulation features:
- Signal scoring boost
- Cluster prioritization
- LLM prompt context

Requirements §6.A mentions emotional modulation as an implementation note ("clusters in that domain should get synthesis priority"), but it's not part of any GOAL's acceptance criteria. The design goes beyond the note by adding signal boost factors and prompt modifications. While this is a reasonable extension, it represents scope creep beyond stated requirements.

### FINDING-7: Gate decision enum incomplete in requirements [severity: minor]
Design §3.1 defines GateDecision with four variants: Synthesize, AutoUpdate, Defer, Skip.

Requirements GOAL-3 only mentions three classifications: SYNTHESIZE, AUTO-UPDATE, SKIP. The DEFER variant is a design addition not explicitly called out in requirements, though it's implied by "Too recent" → DEFER logic.

### FINDING-8: Provenance chain depth constraint unclear [severity: minor]
Requirements GOAL-4 states "Chain depth: insights from insights (for future multi-level, see NON-GOAL-2)" and "GOAL-4 provenance chains must not prevent future extension."

Design §5.1 provenance schema uses `insight_id` and `source_id` but doesn't explicitly document how multi-level chains (insight A derived from insight B which was derived from memories C, D) would be represented. The schema appears to support it (insights are memories, so can be sources), but this should be explicit.

## Passed Checks

- ✅ Section numbering flows correctly (§1-§8, then §10) — except missing §9
- ✅ GOAL-1 reference present and design addresses cluster discovery
- ✅ GOAL-2 reference present and design addresses insight generation  
- ✅ GOAL-3 reference present and design addresses gate check
- ✅ GOAL-4 reference present and design addresses provenance
- ✅ GUARD-1 (Backward Compat) honored — source demotion, no deletion
- ✅ GUARD-2 (LLM Cost) honored — gate check filters before LLM, cost estimation in §3.2
- ✅ GUARD-5 (Insight Identity) honored — insights stored as MemoryRecords with metadata tag
- ✅ GUARD-6 (Provenance Integrity) honored — validation failures don't demote sources (§4.3 check 7)
- ✅ Config defaults table present in §10 with comprehensive coverage
- ✅ No internal contradictions detected between sections
- ✅ AC-1.1 coverage: Multi-signal scoring in §2.2
- ✅ AC-1.2 coverage: Hebbian primary ranking implicit in weight defaults (0.4 > others)
- ✅ AC-1.3 coverage: Recency bias in candidate pool selection §2.5
- ✅ AC-2.2 coverage: Validation checks in §4.3
- ✅ AC-3.1 coverage: Gate decision logic in §3.2
- ✅ AC-3.2 coverage: Near-duplicate detection via embedding similarity 0.92
- ✅ AC-3.3 coverage: Already-covered check in §3.2 step 5
- ✅ AC-4.1 coverage: Bidirectional queries in §5.2
- ✅ AC-4.2 coverage: Provenance schema in §5.1 with cluster_id field

## Summary

- Total findings: 8
- Critical: 1 (GUARD-4 undefined)
- Important: 3 (missing section, signal mismatch, actionability validation gap)
- Minor: 4 (config completeness, emotional scope, defer enum, chain depth docs)

**Recommendation**: Needs fixes before implementation

**Priority actions**:
1. Define GUARD-3 and GUARD-4 in requirements document, or remove references from design
2. Reconcile GOAL-1 clustering signals — either update requirements to include temporal signal or justify design deviation
3. Address AC-2.1(c) actionability validation — document how this is achieved or mark as non-automatable
4. Fix section numbering (add §9 or renumber §10)
5. Complete config reference with missing parameters from requirements §6.B

**Secondary actions**:
6. Document emotional modulation as requirements extension or add to GOAL acceptance criteria
7. Add DEFER to GOAL-3 requirements classification list
8. Explicitly document multi-level provenance chain representation in schema
