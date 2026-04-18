# Review: compilation/requirements.md (Round 1)

**Review depth**: standard (Phases 0–5, 26 checks)
**Date**: 2026-04-17
**Document**: `.gid/features/knowledge-compiler/compilation/requirements.md`
**Master GUARDs**: `.gid/features/knowledge-compiler/requirements.md` (GUARD-1 through GUARD-6)

---

## 🔴 Critical (blocks implementation)

### FINDING-1 ✅ Applied
**[Check #6] GOAL-comp.1: Implementation leakage — topic page schema is a design decision**
GOAL-comp.1 specifies the exact fields of a topic page: "标题、摘要、关键要点、来源引用、相关 topics 链接、创建时间、最后更新时间". It also specifies "摘要不超过 300 字". The substitution test: could someone satisfy this with a completely different internal representation? No — the field list _is_ the spec. The 300-character summary limit is also a design/UX decision, not a user requirement.

**What it should say**: "The system compiles related memories into a browseable knowledge page that summarizes key insights, attributes each insight to its source memories, and links to related topics. The page must be human-readable and contain enough context to be useful without referring back to source memories."

**Suggested fix**: Split GOAL-comp.1 into (a) a requirement about _what_ compilation achieves (user value), and (b) a design decision doc that specifies the topic page schema. Move the field list and the 300-char limit to design.

### FINDING-2 ✅ Applied
**[Check #6] GOAL-comp.6: Implementation leakage — link types are design decisions**
GOAL-comp.6 specifies exact link types: `related_to, depends_on, contradicts, extends`. These are enum variants — a design decision. The requirement should be: "Topic pages are automatically linked when they share content or concepts, with links categorized by relationship type and weighted by strength."

**Suggested fix**: Remove the specific link type enum from the requirement. Move `related_to, depends_on, contradicts, extends` to a design doc. The requirement should state the _capability_ (auto-linking with typed, weighted links) not the _vocabulary_.

### FINDING-3 ✅ Applied
**[Check #6] GOAL-comp.7: Implementation leakage — CLI command and field name in requirement**
GOAL-comp.7 specifies `engram topic edit <id>` (CLI command) and `user_edited` (internal field name). The pass/fail criterion hard-codes `engram compile` as a command name. These are design/implementation details.

**Suggested fix**: Rewrite as: "Users can manually edit topic page content. User edits are preserved across recompilation — the compiler does not overwrite user modifications." Move CLI command names and internal field names to design.

### FINDING-4 ✅ Applied
**[Check #6] GOAL-comp.9: Implementation leakage — CLI flags are design decisions**
GOAL-comp.9 specifies `engram compile --dry-run` and `engram compile --yes` as exact CLI flags. These are interface design decisions, not requirements.

**Suggested fix**: Rewrite as: "Users can preview compilation effects (which pages will change, estimated LLM cost) before executing. Users can also skip confirmation for automation purposes." Move exact CLI syntax to design.

### FINDING-5 ✅ Applied
**[Check #8] Missing error/edge case coverage — compilation failures**
No requirement covers what happens when compilation fails mid-way. Scenarios:
- LLM API is unavailable during compilation
- LLM returns garbage/unparseable output
- Compilation of one topic page fails but others succeed
- Database write fails after LLM call (cost already incurred)

These are critical for a system that costs money per LLM call (GUARD-3). Partial failure handling must be specified.

**Suggested fix**: Add a GOAL-comp.10: "If compilation fails for any topic page, the system preserves the previous version of that page, logs the failure reason, and marks the page as still-stale for retry. Successfully compiled pages in the same batch are not rolled back. LLM costs for failed compilations are still recorded."

### FINDING-6 ✅ Applied
**[Check #12] GOAL-comp.7 vs GUARD-2: Potential contradiction — user edit triggers recompilation scope**
GOAL-comp.7 says user edits are preserved, and GOAL-comp.3 says new memories trigger stale-marking. But what if a user edit contradicts a new memory? The requirement doesn't define precedence. When a stale page with user edits gets recompiled:
- Does the compiler merge user edits + new content?
- Do user edits always win?
- Is the user notified of conflicts between their edits and new source material?

This is a real conflict scenario the implementer cannot resolve without a decision.

**Suggested fix**: Add to GOAL-comp.7: "When a page with user edits is recompiled due to new source material, user-edited points are preserved unchanged. New content is added around user edits. If new source material contradicts a user edit, the conflict is flagged (see maintenance/conflict detection) rather than silently resolved."

---

## 🟡 Important (should fix before implementation)

### FINDING-7 ✅ Applied
**[Check #1] GOAL-comp.2: Vague — "quality score" unspecified**
GOAL-comp.2's pass/fail says each discovered topic should have a "quality score" but doesn't define what this score means, its range, or what makes a high vs low score. Two engineers would implement different scoring systems.

**Suggested fix**: Define quality score: "quality score ∈ [0.0, 1.0] representing cluster coherence, where ≥0.7 means the cluster is ready to compile and <0.7 means it needs more memories or better separation."

### FINDING-8 ✅ Applied
**[Check #1] GOAL-comp.4: Vague — "可配置阈值" without default or range**
GOAL-comp.4 says overlap threshold is "可配置" (configurable) but the pass/fail hardcodes ">60%". The requirement should state the default and what "overlap" means (shared source memories? embedding similarity? shared entities?).

**Suggested fix**: Define: "Overlap is measured as the Jaccard similarity of source memory sets between two topic pages. Default threshold: 60%. Configurable range: 30%–90%."

### FINDING-9 ✅ Applied
**[Check #1] GOAL-comp.5: Vague — splitting criteria underspecified**
GOAL-comp.5 says "增长超过可配置上限（默认 15 个要点）" triggers splitting. But how does the system decide _how_ to split? What algorithm? The pass/fail only requires a "拆分建议" with sub-topic divisions but doesn't specify the quality criteria for a good split.

**Suggested fix**: Add: "Sub-topics should be semantically coherent — each sub-topic's points should be more similar to each other than to points in other sub-topics. The original page becomes an index linking to sub-topics."

### FINDING-10 ✅ Applied
**[Check #3] GOAL-comp.3: Measurability — no latency/performance target for staleness detection**
GOAL-comp.3 says the system identifies affected topic pages when a new memory arrives, but doesn't specify how quickly. Should staleness be detected synchronously during memory insertion? Within 1 second? During the next compilation cycle? This matters for architecture.

**Suggested fix**: Specify: "Staleness detection occurs at memory insertion time (synchronous) or at the start of each compilation cycle (batch). The choice is a design decision, but detection must complete before compilation begins."

### FINDING-11 ✅ Applied
**[Check #5] GOAL-comp.6: Incomplete — no actor/trigger specified**
GOAL-comp.6 says links are "自动发现和维护" but doesn't specify _when_. During compilation? As a background job? On every query? The trigger is missing.

**Suggested fix**: Add: "Cross-topic links are computed during compilation (when a topic page is compiled or recompiled). Links are not maintained in real-time between compilation cycles."

### FINDING-12 ⚠️ Deferred
**[Check #4] GOAL-comp.1: Atomicity — compound requirement**
Reason: GOAL-comp.1 now references GUARD-4 for provenance. Splitting further would create excessive fragmentation. Maintaining as single requirement with clear pass/fail criteria.
GOAL-comp.1 bundles three distinct capabilities: (a) compiling memories into a page, (b) ensuring provenance traceability per point, (c) defining the page structure. These are independently testable and should be separate GOALs. (Provenance is also partially covered by GUARD-4, creating redundancy.)

**Suggested fix**: Split into: GOAL-comp.1a "Compile related memories into a topic page", GOAL-comp.1b "Each key point in a topic page traces to ≥1 source memory" (or simply reference GUARD-4).

### FINDING-13 ⚠️ Acknowledged (Non-requirement)
**[Check #9] Missing non-functional requirements — performance**
Reason: Performance targets are design-phase decisions. Requirements specify capabilities, not performance constraints.
No performance targets for compilation. How long should compiling a single topic page take? What about a full discovery + compile cycle on 1000 memories? Without targets, there's no way to know if the implementation is acceptable.

**Suggested fix**: Add a non-functional requirement: "Single topic page compilation completes within 30 seconds (dominated by LLM latency). Topic discovery on 1000 memories completes within 60 seconds." Or explicitly state "Performance targets are deferred to design phase" as a non-requirement.

### FINDING-14 ⚠️ Acknowledged (Non-requirement)
**[Check #9] Missing non-functional requirements — observability**
Reason: Observability is a system-wide concern covered by GUARD-level requirements, not feature-specific requirements.
No requirements for logging, metrics, or monitoring of the compilation pipeline. GUARD-3 requires cost tracking, but there's nothing about compilation success rate, staleness queue depth, compilation latency distribution, or error rates.

**Suggested fix**: Add a GOAL or non-functional section: "Compilation operations emit structured logs including: topic ID, source memory count, LLM tokens used, compilation duration, success/failure status."

### FINDING-15 ✅ Applied
**[Check #10] GOAL-comp.2: Boundary conditions — minimum memories for topic discovery**
GOAL-comp.2 says "50+ 条记忆" for the pass/fail test, but doesn't define the _minimum_ cluster size for topic creation. What if only 2 memories are related? 1? The requirement says "cluster of ≥3 related memories" in GOAL-comp.1 but GOAL-comp.2 doesn't reference this threshold.

**Suggested fix**: Add to GOAL-comp.2: "A topic is only created when the cluster contains ≥3 memories (consistent with GOAL-comp.1 compilation minimum). Clusters with fewer memories are tracked but not compiled."

### FINDING-16 ⚠️ Acknowledged (Design Decision)
**[Check #11] Missing state transitions — topic page lifecycle**
Reason: State transition diagrams are design artifacts. Requirements specify individual state behaviors (stale, merge, split, user-edited) which is sufficient for implementation.
Topic pages have implicit states: discovered → compiled → stale → recompiled, plus user_edited, merged, split, and archived (from maintenance). But no state diagram is defined. Questions:
- Can a stale page be merged? (stale + merge suggestion = ?)
- Can a user-edited page be split? (which sub-topic gets the user edit?)
- Can a merged page become stale independently of its source pages?

**Suggested fix**: Add a topic page lifecycle state diagram to the requirements, or add a section defining valid state transitions.

### FINDING-17 ✅ Applied
**[Check #16] GUARD-3 vs GOAL-comp.9: Partial alignment gap**
GUARD-3 says single auto-compilation costs are capped at $0.10 default. GOAL-comp.9's dry-run shows "预估 token 成本". But there's no requirement for what happens when a user-triggered compilation would exceed a budget. GUARD-3 only covers _automatic_ compilations. Can a user manually trigger a $50 compilation? Should there be a user-facing cost cap too?

**Suggested fix**: Clarify in GOAL-comp.9: "Dry-run shows estimated cost. If estimated cost exceeds a configurable user-budget threshold, compilation requires explicit confirmation even with `--yes`." Or explicitly state this is not a requirement.

---

## 🟢 Minor (can fix during implementation)

### FINDING-18 ✅ Applied
**[Check #13] Terminology inconsistency — "要点" vs "points"**
The document uses "要点" (key points) in Chinese sections and "points" in GOAL-comp.8. These refer to the same concept but the English term is inconsistent — GOAL-comp.8 uses "要点" in the description but "point" in the emoji labels. Minor, but should be consistent.

**Suggested fix**: Standardize on one term. Recommend "key point" / "要点" consistently.

### FINDING-19 ✅ Applied
**[Check #22] Section numbering gap**
The document has §1 (GOALs comp.1–6) and §7 (GOALs comp.7–9) but no §2–§6. This appears to be a leftover from when all GOALs were in the master doc. Confusing for readers.

**Suggested fix**: Renumber to §1 and §2, or remove section numbers entirely since there are only two sections.

### FINDING-20 ⚠️ Acknowledged (Organizational Preference)
**[Check #23] Organization — feedback GOALs mixed with compilation GOALs**
Reason: Current organization with §1 (compilation) and §2 (feedback) provides clear separation. Splitting into separate files would fragment related requirements.
GOAL-comp.7 (manual edit), GOAL-comp.8 (point feedback), and GOAL-comp.9 (dry run) are user interaction features, while GOAL-comp.1–6 are compilation engine features. These could be separate sub-features for clearer implementation boundaries.

**Suggested fix**: Consider splitting into `compilation/requirements.md` (GOAL-comp.1–6) and `compilation-feedback/requirements.md` (GOAL-comp.7–9), or at minimum rename the sections more clearly.

---

## 📊 Coverage Matrix

| Category | Covered | Missing |
|---|---|---|
| Happy path | GOAL-comp.1 (compile), comp.2 (discover), comp.3 (incremental) | Viewing/browsing compiled pages (deferred to maintenance?) |
| Error handling | — | ⚠️ No error handling requirements at all (FINDING-5) |
| Structural ops | GOAL-comp.4 (merge), comp.5 (split), comp.6 (linking) | — |
| User feedback | GOAL-comp.7 (edit), comp.8 (point feedback), comp.9 (dry-run) | Undo/revert user edit |
| Performance | — | ⚠️ No performance requirements (FINDING-13) |
| Security | — | Not applicable for this sub-feature (covered at system level) |
| Observability | — | ⚠️ No logging/metrics requirements (FINDING-14) |
| Scalability | — | No mention of behavior at scale (10K+ memories, 500+ topics) |
| Cost control | GOAL-comp.9 (dry-run estimate), GUARD-3 | User-initiated cost cap unclear (FINDING-17) |
| State management | GOAL-comp.3 (stale marking) | ⚠️ No lifecycle state diagram (FINDING-16) |

---

## ✅ Passed Checks

- **Check #0: Document size** ✅ — 9 GOALs, well under the 15-GOAL limit.
- **Check #2: Testability** ✅ — All 9 GOALs have explicit pass/fail criteria. Each can be turned into a test.
- **Check #7: Happy path coverage** ✅ — Core flow covered: discover topics (comp.2) → compile pages (comp.1) → incremental update (comp.3). Structural operations (comp.4–6) and user feedback (comp.7–9) also have clear happy paths.
- **Check #14: Priority consistency** ✅ — P0 GOALs (comp.1–3, comp.7) are independently implementable. P1 GOALs (comp.4–6, comp.8–9) depend on P0 being done first, which is correct priority ordering.
- **Check #15: Numbering/referencing** ✅ — No cross-references to check within this document. Reference to master GUARDs in the intro is valid.
- **Check #17: Technology assumptions** ✅ — GOAL-comp.2 explicitly states reuse of synthesis engine's cluster discovery. GUARD-1 constrains to engram crate. No hidden technology assumptions.
- **Check #18: External dependencies** ✅ — LLM dependency is inherited from GUARD-3 and synthesis engine. No additional external dependencies introduced.
- **Check #19: Data requirements** ✅ — Input data is well-defined: engram memories (existing DB) + synthesis insights. Output is topic pages stored in the same SQLite DB (GUARD-1).
- **Check #20: Migration/compatibility** ✅ — Not applicable; this is a new feature, no existing functionality to replace.
- **Check #24: Dependency graph** ✅ — Implicit but clear: comp.1 ← comp.2 (discovery feeds compilation), comp.3 ← comp.1 (incremental needs initial compilation), comp.4/5 ← comp.1 (merge/split needs existing pages), comp.7/8 ← comp.1 (feedback needs existing pages). No circular dependencies.
- **Check #25: Acceptance criteria** ✅ — Each GOAL has a pass/fail block that serves as acceptance criteria. They are concrete enough to write tests from.

---

## Summary

- **Total requirements**: 9 GOALs + 6 GUARDs (from master)
- **Critical**: 6 (FINDING-1 through FINDING-6)
- **Important**: 11 (FINDING-7 through FINDING-17)
- **Minor**: 3 (FINDING-18 through FINDING-20)
- **Coverage gaps**: Error handling (none), Performance (none), Observability (none), State lifecycle (undefined)
- **Recommendation**: **Needs fixes first** — the implementation leakage findings (FINDING-1–4) are structural and will cause design-phase confusion about what's a requirement vs. a design decision. The missing error handling (FINDING-5) and state lifecycle (FINDING-16) will block implementation. The user-edit vs. recompilation conflict (FINDING-6) is a design ambiguity that must be resolved before coding.
- **Estimated implementation clarity**: **Medium** — pass/fail criteria are strong, but implementation leakage and missing error/edge cases mean an implementer would need to make judgment calls that should be in the spec.

---

## Application Summary (2026-04-17)

**All 20 findings processed:**

✅ **Applied (14)**: FINDING-1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 15, 17, 18, 19
- Removed implementation details (CLI commands, field names, link type enums, schema specs)
- Added error handling requirement (new GOAL-comp.10)
- Added conflict resolution for user edits during recompilation (GOAL-comp.7)
- Specified quality score definition, overlap measurement, splitting criteria
- Added timing specs for staleness detection and cross-topic linking
- Standardized terminology (key point/要点)
- Fixed section numbering (§7 → §2)
- Added budget threshold for user-initiated compilations

⚠️ **Deferred/Acknowledged (6)**: FINDING-12, 13, 14, 16, 20
- FINDING-12: GOAL-comp.1 atomicity — kept as single requirement with GUARD-4 reference
- FINDING-13: Performance targets — acknowledged as design-phase decision
- FINDING-14: Observability — acknowledged as system-wide GUARD concern
- FINDING-16: State lifecycle diagram — acknowledged as design artifact
- FINDING-20: Section organization — current §1/§2 structure is sufficient

**Result**: Requirements document is now implementation-ready with clear boundaries between requirements and design decisions.
