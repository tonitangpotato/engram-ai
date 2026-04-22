# Review: Memory Supersession Requirements

**Review depth**: standard (Phases 0–5, checks 0–25)
**Document**: `.gid/features/supersession/requirements.md`
**Reviewed**: 2026-04-19

---

## 🔴 Critical (blocks implementation)

### FINDING-1: [Check #6] GOAL-ss.1: References non-existent `EngineState` type
GOAL-ss.1 specifies `supersede(old_id, new_id)` as a method on `EngineState`. The codebase has no `EngineState` struct. The main memory struct is `Memory` (in `src/memory.rs`). An implementer cannot start work without knowing where to put this method.

**Suggested fix**: Replace "provides a `supersede(old_id, new_id)` method on `EngineState`" with "provides a `supersede(old_id, new_id)` method on `Memory`" — or reword to be implementation-neutral: "The system provides a supersede operation that takes an old memory ID and a new memory ID."

### FINDING-2: [Check #6] GOAL-ss.3: Implementation leakage — specifies CLI argument format
GOAL-ss.3 prescribes the exact CLI syntax `engram correct <old_id> "new content"` and specifies internal steps (creates memory, calls supersede, outputs ID). This is design, not requirements. Two engineers could satisfy "user can correct a memory by providing the old ID and new text" with different CLI designs. The 3-step numbered process is an implementation spec.

**Suggested fix**: Rewrite as: "A user can correct a single memory by specifying its ID and providing replacement content. The old memory is superseded and a new memory is created with the corrected content, inheriting at minimum the same memory_type and namespace. The user is informed of the new memory ID and the supersession." Move the CLI syntax to a design doc.

### FINDING-3: [Check #6] GOAL-ss.5: Implementation leakage — specifies CLI flags and workflow
Same issue as FINDING-2. `engram correct-bulk --query "wrong fact" "corrected content"` and the `--yes` flag are design decisions. The requirement should be: "A user can correct multiple memories matching a search query in a single operation, with a confirmation step (skippable)."

**Suggested fix**: Strip CLI syntax, keep behavioral requirement. Move exact CLI flag names to design doc.

### FINDING-4: [Check #6] GOAL-ss.6: Implementation leakage — specifies algorithm details
GOAL-ss.6 prescribes: embedding similarity threshold (>0.85), specific negation keywords ("not", "doesn't", "no longer", "不", "没有", "不再", "actually", "correction:"), and the detection algorithm (high similarity AND negation signals). These are design/algorithm decisions, not requirements. The requirement is: "The system can optionally detect when newly stored content contradicts existing memories and suggest supersession candidates, without auto-applying."

The hardcoded 0.85 threshold and specific keyword lists will cause review churn if the design phase discovers better values. These belong in configuration or design docs.

**Suggested fix**: Rewrite as: "When storing a new memory, the system optionally detects potential contradictions with existing memories using embedding similarity and textual signals. Detection thresholds and signal patterns are configurable. When a contradiction is detected, the system returns candidate old memory IDs for the caller to review — it does not auto-supersede."

### FINDING-5: [Check #12] GOAL-ss.6 vs GUARD-ss.2: Tension between auto-detection and backward compatibility
GOAL-ss.6 introduces contradiction detection on store, which overlaps with the existing `contradicted_by` field's purpose. GUARD-ss.2 says the two mechanisms are "independent." But GOAL-ss.6's detection could logically populate `contradicted_by` instead of `superseded_by` — the requirements don't clarify why a new mechanism is needed vs. populating the existing field. An implementer may question whether GOAL-ss.6 should set `contradicted_by` (existing field) or suggest supersession (new field) or both.

**Suggested fix**: Add explicit clarification: "GOAL-ss.6 suggests supersession, not contradiction. The existing `contradicted_by` field remains unpopulated by automatic detection. The distinction: `contradicted_by` applies a scoring penalty; `superseded_by` fully excludes from recall. GOAL-ss.6 recommends the stronger mechanism (supersession) because penalty-based downranking is insufficient (see Overview)."

---

## 🟡 Important (should fix before implementation)

### FINDING-6: [Check #5] GOAL-ss.1: Missing error behavior specification
GOAL-ss.1 says "The new memory `new_id` must exist" but does not specify: What if `old_id` doesn't exist? What if `old_id` is already superseded? What if `old_id == new_id`? What return type / error type is expected? Each of these is a question an implementer must answer.

**Suggested fix**: Add: "If `old_id` does not exist, the operation returns an error. If `old_id` is already superseded, the operation updates the supersession link to point to `new_id` (last-write-wins). Self-supersession (`old_id == new_id`) is an error."

### FINDING-7: [Check #5] GOAL-ss.4: Missing error behavior for bulk operation
`supersede_bulk(old_ids, new_id)`: What if some IDs don't exist? Is it all-or-nothing (transactional) or best-effort? What if `old_ids` is empty? What is the return value — count of superseded? List of failures?

**Suggested fix**: Specify: "The operation is transactional — either all IDs are superseded or none are. If any `old_id` does not exist, the entire operation fails with an error listing the invalid IDs. Empty `old_ids` is a no-op (returns success with count 0)."

### FINDING-8: [Check #10] GOAL-ss.7: Chain depth boundary not specified
GOAL-ss.7 says chains are resolved transitively (A→B→C). What's the maximum chain depth? What happens with a chain of 1000? Is there cycle detection (A→B→A)? A cycle would cause infinite recursion in a naive implementation.

**Suggested fix**: Add: "Chain resolution must detect cycles. If a cycle is detected (A supersedes B, B supersedes A), the system treats all memories in the cycle as superseded (none appear in recall) and logs a warning. Chain depth is unbounded but expected to be <10 in practice; resolution must not use recursion (use iterative traversal)."

### FINDING-9: [Check #5] GOAL-ss.3: Namespace inheritance underspecified
GOAL-ss.3 says the new memory gets "same memory_type, namespace, importance as the old one, or higher importance." "Or higher importance" is ambiguous — who decides? Is it max(old_importance, some_default)? Or does the user specify? Also, `MemoryRecord` does not have a `namespace` field — namespace is a storage-layer concept passed separately to `Storage::add()`. The requirement assumes namespace is a memory attribute.

**Suggested fix**: Clarify: "The new memory inherits memory_type from the old memory and is stored in the same namespace. Importance is set to max(old_importance, 0.5) unless the user specifies a value. The user may optionally override memory_type and importance."

### FINDING-10: [Check #8] GOAL-ss.8: Undo semantics for chains not specified
`unsupersede(old_id)` clears `superseded_by`. But what if A→B→C and you unsupersede B? Now B is active, but A still points to B which is no longer superseded. Is A still superseded? (Yes, because A.superseded_by = B, and B exists.) But the user intent might be to restore the whole chain. This needs clarification.

**Suggested fix**: Add: "Undo applies only to the specified memory. If A is superseded by B and B is superseded by C, `unsupersede(B)` restores B to active recall. A remains superseded (A.superseded_by = B still set). To fully restore A, `unsupersede(A)` must also be called."

### FINDING-11: [Check #13] Terminology: `supersede` vs `superseded_by` vs `correct` vs `contradicted_by`
Four overlapping terms: "supersede" (new mechanism), "correct" (CLI command), "contradict" (existing field), and "supercede" (common misspelling to watch for). The doc is consistent internally but the relationship to `contradicted_by` could be clearer for implementers unfamiliar with the history. 

**Suggested fix**: Add a terminology section: "**Supersession** = new mechanism (filter/exclude). **Contradiction** = existing mechanism (score penalty via `contradicted_by`). **Correction** = user-facing action that triggers supersession. These are distinct: correction is the user intent, supersession is the mechanism, contradiction is the legacy mechanism."

### FINDING-12: [Check #19] GOAL-ss.6: Embedding dependency not addressed
GOAL-ss.6 requires embedding similarity (>0.85), but the `Memory` struct has `embedding: Option<EmbeddingProvider>`. When no embedding provider is configured, what happens? Is GOAL-ss.6 simply disabled? This should be explicit.

**Suggested fix**: Add: "Auto-detection requires an embedding provider. If no embedding provider is configured, auto-detection is silently disabled (no error, no suggestions)."

### FINDING-13: [Check #3] GUARD-ss.4: Vague performance requirement
"Must not add measurable latency" is not measurable itself. What counts as "measurable"? <1ms? <5ms? This is essentially unfalsifiable in a test.

**Suggested fix**: Replace with: "Pre-filter adds <1ms overhead per recall operation for databases up to 100K memories, as measured by benchmarks comparing recall latency with and without the supersession filter."

---

## 🟢 Minor (can fix during implementation)

### FINDING-14: [Check #22] Numbering scheme uses `ss.N` prefix
The `ss.` prefix is fine for namespacing within the feature. No gaps detected (1–8 for GOALs, 1–4 for GUARDs). No issues.

### FINDING-15: [Check #23] Organization is clear
GOALs are ordered by priority (P0 first, then P1, then P2). GUARDs are grouped separately. Out of scope and dependencies are documented. Structure is good.

### FINDING-16: [Check #4] GOAL-ss.5: Compound requirement
GOAL-ss.5 describes 5 steps (search, show, confirm, create, supersede, report). While they form a single user flow, the confirmation UX (step 2) could be split from the core operation for independent testability.

**Suggested fix**: Consider splitting into: (a) bulk search for supersession candidates, (b) bulk supersede with confirmation. Minor — can be resolved during design.

### FINDING-17: [Check #15] No cross-references between GOALs
GOAL-ss.5 depends on GOAL-ss.4 (bulk supersession) but doesn't reference it. GOAL-ss.3 implicitly uses GOAL-ss.1 (supersede API) but doesn't reference it. Making dependencies explicit aids implementation ordering.

**Suggested fix**: Add "Depends on: GOAL-ss.1" to GOAL-ss.3, and "Depends on: GOAL-ss.4" to GOAL-ss.5.

---

## 📊 Coverage Matrix

| Category | Covered | Missing |
|---|---|---|
| Happy path | GOAL-ss.1, ss.2, ss.3, ss.4, ss.5 | — |
| Error handling | Partial (GOAL-ss.1 mentions new_id must exist) | ⚠️ old_id not found, already superseded, self-supersession, bulk partial failure, cycle detection |
| Performance | GUARD-ss.4 | Vague — no concrete threshold (see FINDING-13) |
| Security | — | ⚠️ No security requirements. Can any agent supersede any memory? ACL checks on supersede? Namespace boundaries? |
| Observability | — | ⚠️ No logging/metrics requirements. Should supersession events be logged? Audit trail? |
| Scalability | — | Not addressed, but likely acceptable for a local-first library |
| Migration | GUARD-ss.3 | Covered adequately |
| Backward compat | GUARD-ss.2 | Covered |
| Recovery/undo | GOAL-ss.8 | Chain undo semantics unclear (FINDING-10) |
| Non-goals | Out of Scope section | Well-specified ✅ |

**Notable gap — Security**: The existing codebase has ACL/namespace permission checks (`agent_id`, namespace-scoped operations). Supersession crosses memory boundaries — can an agent in namespace A supersede a memory in namespace B? This needs a requirement or explicit non-requirement.

**Notable gap — Observability**: For a feature that permanently alters recall behavior, there should be at minimum a way to list all superseded memories and their supersessors. This aids debugging "why doesn't memory X appear in recall anymore?"

---

## ✅ Passed Checks

- **Check #0**: Document size ✅ — 8 GOALs, 4 GUARDs. Well under the 15-GOAL limit.
- **Check #1**: Specificity ✅ — 7/8 GOALs have concrete, specific descriptions. GOAL-ss.6 is specific but over-specified (implementation leakage, see FINDING-4).
- **Check #2**: Testability ✅ — All 8 GOALs have explicit acceptance criteria with pass/fail conditions.
- **Check #3**: Measurability — 7/8 pass. GUARD-ss.4 fails (FINDING-13). GOALs are behavioral (pass/fail), not quantitative, which is appropriate.
- **Check #4**: Atomicity ✅ — 7/8 GOALs are atomic. GOAL-ss.5 is borderline compound (FINDING-16, minor).
- **Check #5**: Completeness — 5/8 GOALs fully specify actor/trigger/behavior/outcome. GOAL-ss.1, ss.4, ss.8 missing error cases (FINDING-6, 7, 10).
- **Check #6**: Implementation leakage — 4/8 GOALs leak implementation details (FINDING-1, 2, 3, 4). This is the primary issue.
- **Check #7**: Happy path ✅ — All normal flows covered: single correct, bulk correct, manual supersede, auto-detect, undo.
- **Check #8**: Error/edge cases — Partially covered. Significant gaps in error handling (see Coverage Matrix).
- **Check #9**: Non-functional requirements — Performance partially covered. Security and observability missing entirely.
- **Check #10**: Boundary conditions — Chain depth unbounded (FINDING-8). Empty inputs not specified for bulk operations (FINDING-7).
- **Check #11**: State transitions ✅ — Memory states are simple: active → superseded (via supersede) → active (via unsupersede). No unreachable states. Chain resolution specified in GOAL-ss.7.
- **Check #12**: Internal consistency — One tension found (FINDING-5). No hard contradictions.
- **Check #13**: Terminology — Mostly consistent. Clarification recommended (FINDING-11).
- **Check #14**: Priority consistency ✅ — P0 items (GOAL-ss.1, ss.2, ss.3) are independent. P1 items depend on P0 (correct ordering). P2 (ss.6) is independent of P1.
- **Check #15**: Cross-references — No explicit cross-references; implicit dependencies exist (FINDING-17).
- **Check #16**: GUARDs vs GOALs alignment ✅ — No GUARD makes any GOAL unimplementable. GUARD-ss.1 (no deletion) is compatible with all GOALs. GUARD-ss.3 (safe migration) is compatible with adding `superseded_by` field.
- **Check #17**: Technology assumptions ✅ — SQLite assumed (consistent with existing codebase using rusqlite). GUARD-ss.3 explicitly mentions `ALTER TABLE`. Acceptable given the project context.
- **Check #18**: External dependencies ✅ — GOAL-ss.6 depends on embedding provider, noted as optional in codebase. No new external dependencies introduced.
- **Check #19**: Data requirements — Partially covered. Embedding availability for GOAL-ss.6 not addressed (FINDING-12).
- **Check #20**: Migration ✅ — GUARD-ss.3 covers schema migration. No data transformation needed (new field defaults to empty).
- **Check #21**: Scope boundaries ✅ — Out of Scope section is explicit and well-chosen: no auto-supersession, no UI, no LLM detection, no versioning.
- **Check #22**: Unique identifiers ✅ — All 8 GOALs and 4 GUARDs have unique IDs. No gaps.
- **Check #23**: Organization ✅ — Grouped by type (GOALs/GUARDs), ordered by priority within GOALs.
- **Check #24**: Dependency graph — Implicit only (FINDING-17). No circular dependencies detected.
- **Check #25**: Acceptance criteria ✅ — Every GOAL has an explicit acceptance test. Quality is high — each describes concrete steps and expected outcomes.

---

## Summary

- **Total requirements**: 8 GOALs, 4 GUARDs
- **Critical**: 5 (FINDING-1 through FINDING-5)
- **Important**: 8 (FINDING-6 through FINDING-13)
- **Minor**: 4 (FINDING-14 through FINDING-17)
- **Coverage gaps**: Security (ACL for supersession), Observability (audit/listing), Error handling (multiple edge cases)
- **Recommendation**: **Needs fixes first** — the implementation leakage issues (FINDING-2,3,4) will cause design-phase conflicts, and the `EngineState` reference (FINDING-1) is simply wrong. Error behavior gaps (FINDING-6,7,8) will generate implementation questions. Fix criticals and importants, then proceed to design.
- **Estimated implementation clarity**: **Medium** — acceptance criteria are excellent, but error paths and the contradiction/supersession boundary need clarification before an implementer can work without asking questions.
