# Requirements: Memory Supersession

**Status**: Partial — data model implemented (`types.rs` superseded_by field, `memory.rs::supersede()`, `association/*` respects it); automatic supersession detection on write **not** implemented. Currently requires explicit API call. Will emerge naturally once ISS-016 triple extraction + ISS-003 dedup + ISS-007 confidence are all live (see issues-index.md §Supersedes).
**Last reviewed**: 2026-04-20

## Overview

When a user corrects a fact ("engram doesn't use MCP" correcting an earlier "engram uses MCP"), the old memory must be **excluded** from recall results — not just downranked. The current system has `contradicted_by` fields and `contradiction_penalty` in ACT-R scoring, but nothing ever populates `contradicted_by`, so old/wrong memories persist indefinitely and outnumber corrections.

The core problem: **降权不够，要过滤。** A 3.0 penalty doesn't help when 50 old memories with the wrong fact outvote 1 correction. Superseded memories must be removed from the candidate set before scoring begins.

## Terminology

- **Supersession** = the new mechanism introduced by this feature. A superseded memory is **filtered** (excluded from recall candidate set entirely). Supersession is the stronger mechanism.
- **Contradiction** = the existing legacy mechanism. A contradicted memory receives a **score penalty** via the `contradicted_by` field and `contradiction_penalty` in ACT-R scoring. The penalty reduces ranking but does not exclude.
- **Correction** = the user-facing action that triggers supersession. The user provides an old memory ID and new content; the system creates a new memory and supersedes the old one. Correction is the user intent; supersession is the mechanism.

These are distinct: correction is the user intent, supersession is the mechanism, contradiction is the legacy mechanism.

## Priority Levels

- **P0**: Core — required for the mechanism to function
- **P1**: Important — needed for production-quality operation
- **P2**: Enhancement — improves UX or catches more cases

## Guard Severity

- **hard**: Violation = data corruption or wrong results guaranteed
- **soft**: Violation = degraded quality, acceptable temporarily

---

## GOALs

### GOAL-ss.1: Supersede API [P0]

The system provides a supersede operation that takes an old memory ID and a new memory ID, and marks the old memory as superseded by the new one. After this operation, the old memory will not appear in any recall results (recall, recall_from_namespace, hybrid_recall, recall_with_associations). The new memory must exist. The old memory is not deleted — it remains in storage for audit/history but is filtered from all recall pipelines.

**Error cases**:
- If `old_id` does not exist, the operation returns an error.
- If `old_id` is already superseded, the operation updates the supersession link to point to `new_id` (last-write-wins).
- Self-supersession (`old_id == new_id`) is an error.

**Acceptance**: Call `supersede(A, B)`. Recall with a query that previously returned A. A must not appear in results. B appears if relevant.

### GOAL-ss.2: Recall Pre-Filter [P0]

All recall paths (recall, recall_from_namespace, hybrid_recall, recall_recent, recall_with_associations, recall_associated) apply a pre-filter **before** scoring/ranking that excludes any memory whose `superseded_by` field is non-empty. This is a filter (exclude from candidate set), not a penalty (reduce score).

**Acceptance**: Store 10 memories on topic X. Supersede 5 of them. Recall topic X with limit=10. Only the 5 non-superseded memories appear.

### GOAL-ss.3: Single Memory Correction [P0]

*Depends on: GOAL-ss.1*

A user can correct a single memory by specifying its ID and providing replacement content. The old memory is superseded and a new memory is created with the corrected content. The new memory inherits memory_type from the old memory and is stored in the same namespace. Importance is set to max(old_importance, 0.5) unless the user specifies a value. The user may optionally override memory_type and importance. The user is informed of the new memory ID and the supersession.

**Acceptance**: Correct a memory by providing its ID and new content. Recall the topic. Old memory absent, new memory present with inherited memory_type and namespace.

### GOAL-ss.4: Bulk Supersession [P1]

*Depends on: GOAL-ss.1*

The system provides a bulk supersede operation that supersedes multiple old memories with a single new one. This handles the common case where the same wrong fact was stored N times (e.g., 50 memories saying "engram uses MCP") and all need to be superseded by one correction.

The operation is transactional — either all IDs are superseded or none are. If any `old_id` does not exist, the entire operation fails with an error listing the invalid IDs. Empty `old_ids` is a no-op (returns success with count 0).

**Acceptance**: Store 20 memories with content "X is true". Store 1 memory "X is false". Call bulk supersede with all 20 IDs and the correction ID. Recall "X". Only the correction appears.

### GOAL-ss.5: Bulk Memory Correction [P1]

*Depends on: GOAL-ss.4*

A user can correct multiple memories matching a search query in a single operation, with a confirmation step (skippable). The system searches for matching memories, presents them for review, creates one new memory with the corrected content, supersedes all matching memories with the new one, and reports how many memories were superseded.

> **Note**: This GOAL describes a compound user flow (search → confirm → create → supersede → report). The confirmation UX may be split from the core operation during design for independent testability.

**Acceptance**: Store 15 memories about "X uses Y". Bulk-correct with query "X uses Y" and replacement "X does not use Y". Recall "X uses Y". Only the correction appears.

### GOAL-ss.6: Auto-Detection on Store [P2]

When storing a new memory, the system optionally detects potential contradictions with existing memories using embedding similarity and textual signals. Detection thresholds and signal patterns are configurable. When a contradiction is detected, the system returns candidate old memory IDs for the caller to review — it does not auto-supersede.

Auto-detection requires an embedding provider. If no embedding provider is configured, auto-detection is silently disabled (no error, no suggestions).

**Why supersession, not contradiction**: GOAL-ss.6 suggests supersession, not contradiction. The existing `contradicted_by` field remains unpopulated by automatic detection. The distinction: `contradicted_by` applies a scoring penalty; `superseded_by` fully excludes from recall. GOAL-ss.6 recommends the stronger mechanism (supersession) because penalty-based downranking is insufficient when wrong facts outnumber corrections (see Overview).

**Acceptance**: Store "engram uses MCP". Then store "engram does not use MCP". The second store returns a suggestion: "Found potentially superseded memory: [old_id]". The old memory is NOT auto-superseded (requires explicit call).

### GOAL-ss.7: Supersession Chain Resolution [P1]

*Depends on: GOAL-ss.1*

If memory A is superseded by B, and B is later superseded by C, then A is also effectively superseded (transitively). The recall pre-filter handles chains: any memory reachable through `superseded_by` links to a non-superseded head is filtered. Only the head (latest correction) appears in results.

Chain resolution must detect cycles. If a cycle is detected (e.g., A supersedes B, B supersedes A), the system treats all memories in the cycle as superseded (none appear in recall) and logs a warning. Chain depth is unbounded but expected to be <10 in practice; resolution must not use recursion (use iterative traversal).

**Acceptance**: Create A, B, C. Supersede A→B, then B→C. Recall returns only C. A and B are both filtered.

### GOAL-ss.8: Undo Supersession [P1]

*Depends on: GOAL-ss.1*

The system provides an unsupersede operation that clears the `superseded_by` field for a specified memory, restoring it to the active candidate set. This is the recovery path for mistaken supersessions.

Undo applies only to the specified memory. If A is superseded by B and B is superseded by C, unsuperseding B restores B to active recall. A remains superseded (A's `superseded_by` still points to B). To fully restore A, A must also be unsuperseded separately.

**Acceptance**: Supersede A→B. Recall: A absent. Unsupersede A. Recall: A present again.

---

## GUARDs

### GUARD-ss.1: No Data Deletion [hard]

Supersession MUST NOT delete any memory from storage. Superseded memories remain in the database with all original fields intact. Only their visibility in recall results changes.

### GUARD-ss.2: Backward Compatibility [hard]

The existing `contradicted_by` field and `contradiction_penalty` scoring continue to work unchanged. Supersession is a new, parallel mechanism. Memories with `contradicted_by` set (if any exist) still receive the ACT-R penalty. The two mechanisms are independent: a memory can be contradicted (penalty) or superseded (filtered) or both.

### GUARD-ss.3: Storage Schema Migration [soft]

Adding the `superseded_by` column must use safe migration: `ALTER TABLE ... ADD COLUMN ... DEFAULT ''`. Existing databases with 15,000+ memories must not require re-indexing or data transformation. The migration must be idempotent (running it twice is safe).

### GUARD-ss.4: Performance [soft]

The pre-filter adds <1ms overhead per recall operation for databases up to 100K memories, as measured by benchmarks comparing recall latency with and without the supersession filter. Since it's a simple field check (is `superseded_by` empty?), it should be implementable as a SQL WHERE clause or in-memory filter with O(1) per record overhead.

---

## Security

### SEC-ss.1: Namespace-Scoped Supersession

Supersession is namespace-scoped. A caller can only supersede memories within namespaces they have access to. Cross-namespace supersession (superseding a memory in namespace A from namespace B) requires explicit permission or access to both namespaces. The supersede operation must validate that the caller has appropriate access to the namespace(s) containing both `old_id` and `new_id`.

---

## Observability

### OBS-ss.1: List Superseded Memories

The system provides a way to list all superseded memories and their supersessors (the memories that replaced them). This aids debugging when a user asks "why doesn't memory X appear in recall anymore?" The listing includes: the superseded memory ID, its content (or summary), the superseding memory ID, and when the supersession occurred.

---

## Out of Scope

- **Auto-supersession without user confirmation** — too risky for factual corrections
- **Conflict resolution UI** — this is a library, not an application
- **Semantic contradiction detection via LLM** — GOAL-ss.6 uses heuristics only; LLM-based detection is a future enhancement
- **Versioning / full history tracking** — supersession is a simple "old→new" link, not a version control system

---

## Dependencies

- Existing DB schema (`memories` table with `contradicted_by` column)
- Existing memory API (recall, store methods)
- Existing CLI (clap-based, `main.rs`)
