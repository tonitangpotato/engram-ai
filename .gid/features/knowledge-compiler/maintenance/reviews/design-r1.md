# Design Review R1: Knowledge Maintenance, Access & Privacy

**Reviewer**: RustClaw  
**Date**: 2026-04-17  
**Document**: maintenance/design.md  
**Depth**: standard (Phase 0-5)  
**Score**: 9/10

---

## Summary

Strongest of the three design docs. 7 components (within limit), all 12 GOALs covered, clean data flow diagram, and solid GUARD compliance through PrivacyGuard. The maintenance design is well-structured and implementation-ready. A few minor gaps.

---

## Findings

### FINDING-1: GOAL-maint.5 vs GOAL-maint.5b conflation [✅ Applied]

**Phase 3 — Check 15 (Numbering/referencing)**

Requirements define two separate GOALs:
- GOAL-maint.5: "Knowledge Health Report" (P2) — system-wide health metrics
- GOAL-maint.5b: "Maintenance Operation Summary" (P1) — per-operation output summary

The design's §2.3 HealthAuditor covers maint.5 well but doesn't explicitly address maint.5b (per-operation summaries). The traceability matrix maps both to HealthAuditor, but maint.5b is about operation-level output (compile cycle summary with counts, timing, token cost), not system health.

**Fix**: Add maint.5b to MaintenanceApi or the compile orchestrator — each maintenance operation returns a `OperationSummary` struct with counts/timing/cost.

### FINDING-2: GOAL-maint.6 (Knowledge-Aware Recall) not in design [✅ Applied]

**Phase 2 — Check 7 (Coverage)**

GOAL-maint.6 requires: "engram recall prioritizes topic pages over fragment memories when query matches a topic."

The design's §2.4 ExportEngine maps to maint.6, but that's wrong — maint.6 is about **recall integration**, not export. Knowledge-aware recall means modifying `recall()` to also search compiled topic pages and boost their ranking.

This is a cross-cutting concern that touches `memory.rs` (recall path), not just maintenance components.

**Fix**: Add a brief section on recall integration — how topic pages participate in hybrid search scoring. Could be a method on MaintenanceApi (`query()` is already there) or an integration hook in the recall pipeline.

### FINDING-3: Decay model vs requirements mismatch [✅ Applied]

**Phase 2 — Check 10 (Boundary conditions)**

Requirements GOAL-maint.1 says: "活跃度评分基于其源记忆的 ACT-R 激活度加权平均" — decay is based on source memories' ACT-R activation, not independent time/access signals.

The design's §2.1 DecayEngine uses its own half-life formula (`base_score * 0.5^(elapsed / half_life)`) with two independent signals (time decay + access decay). This diverges from the requirement to derive activity from **source memory ACT-R activation**.

**Fix**: Either:
A. Change design to compute topic freshness from source memories' ACT-R scores (as requirements specify)
B. Update requirements to allow the independent decay model (if that's intentionally better)

Recommendation: Option A — ACT-R activation already captures time decay and access patterns. Reusing it is simpler and more consistent with engram's existing model.

### FINDING-4: PrivacyGuard redaction depends on entity extraction [✅ Applied]

**Phase 4 — Check 14 (Coupling)**

§2.7 PrivacyGuard's redaction pipeline uses entity detection for sensitive patterns. This implicitly depends on the entity extraction system (B1 in the GID graph, not yet implemented).

The design should note this dependency explicitly and specify fallback behavior if entity extraction isn't available (regex-only patterns? skip redaction?).

**Fix**: Add a note that PrivacyGuard's entity-based redaction has a dependency on entity index (or at minimum uses regex patterns as a standalone fallback).

### FINDING-5: MaintenanceLock file path not specified [✅ Applied]

**Phase 4 — Check 15 (Configuration)**

§3 describes file-based leader election but doesn't specify:
- Lock file location (alongside the DB? configurable?)
- Lock file format
- What happens when the lock path is not writable

**Fix**: Specify lock file path as `{db_dir}/.engram-maintenance.lock` and document the failure mode (can't acquire lock → skip maintenance cycle with warning).

### FINDING-6: GUARD-3 (LLM Cost Awareness) not addressed [✅ Applied]

**Phase 4 — Check 13 (Architecture consistency)**

Similar to compilation design — no component tracks LLM costs for maintenance operations (conflict detection uses LLM). GUARD-3 requires cost estimation and configurable limits.

**Fix**: ConflictDetector's LLM calls should estimate tokens before sending and respect the budget threshold. Add a brief note.

---

## Score Breakdown

| Area | Score | Notes |
|------|-------|-------|
| Structure & completeness | 9/10 | Clean 7-component design |
| GOAL traceability | 8/10 | maint.6 mapping wrong, maint.5b thin |
| GUARD compliance | 8/10 | GUARD-3 missing, rest covered |
| Logic correctness | 9/10 | Decay/conflict/audit logic sound |
| Edge cases | 9/10 | Good error handling throughout |
| Trade-offs | 8/10 | Implicit but reasonable |

**Overall: 9/10** — Clean, implementation-ready. Minor traceability fixes needed.
