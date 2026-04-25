# Requirements Review: Engram v0.3 (r1)

> **Reviewer:** RustClaw (self-review, not delegated)
> **Date:** 2026-04-24
> **Applied:** 2026-04-24 (all applicable findings applied in same session)
> **Scope:** Master `.gid/docs/requirements-v03.md` + 4 feature docs (v03-graph-layer, v03-resolution, v03-retrieval, v03-migration)
> **Review skill:** review-requirements (27 checks × 4 feature docs + master)
> **Source docs:** DESIGN-v0.3.md (1029 lines, §0-§12)

## Apply Summary

- **Applied:** 13/14 findings
- **Dropped:** 1/14 (FINDING-4 — misread: GOAL-1.9 does not contain "auditable"; the actual GOAL-1.9 is fine as-is)
- **New feature created:** `v03-benchmarks` (8 GOALs, 5 P0) for ship-gate coverage
- **Resolved open question:** Q7 topic reconciliation → preserve-plus-resynthesize
- **Totals updated:** 5 features / 60 GOALs (was 4 features / 50 GOALs)

---

## Summary

**Total findings: 14** (3 🔴 Critical / 8 🟡 Important / 3 🟢 Minor)

The four feature docs are **well-scoped, WHAT/HOW discipline is mostly good**, numbering is clean, and GUARD references (not restatements) work as intended. The split decision (graph-layer / resolution / retrieval / migration) holds up — no GOAL feels like it belongs in a different feature.

**Main gap categories:**
1. **Missing ship-gate coverage** — DESIGN §11 success criteria (LOCOMO ≥68.5%, LongMemEval ≥v0.2+15pp, ≤3 LLM calls, 280 tests pass) don't appear as GOALs anywhere. These are the ship/no-ship gates.
2. **Backward compatibility underspecified** — DESIGN §7.3 promises exact v0.2 API signatures stay working; no feature has a GOAL for this.
3. **A handful of measurability issues** — some GOALs use "reasonable", "bounded", or unmeasurable phrasing.
4. **Retrieval has 14 GOALs (over soft limit of ~12)**, but all map to distinct design sections — acceptable.

Nothing here blocks moving forward with the graph/task-generation phase *if* the 🔴 findings get addressed first.

---

## 🔴 Critical findings (must fix before proceeding)

### FINDING-1 🔴 ✅ Applied — New feature `v03-benchmarks` created

**Location:** No feature currently owns §11 success criteria.
**Issue:** DESIGN §11 lists concrete ship gates that are ALL measurable:
- LOCOMO overall ≥ 68.5%
- LOCOMO temporal ≥ Graphiti's number
- LongMemEval overall ≥ v0.2 baseline + 15pp
- Avg LLM calls per episode ≤ 3 over N=500 benchmark episodes
- All ~280 v0.2 tests still pass after migration
- Interoceptive/metacognition/affect features demonstrably affect retrieval ranking (regression test)

None of these appear as GOALs. The ≤3 LLM calls criterion is softly present as GUARD-12 (telemetry warning), but §11 makes it a ship gate, which is stronger.

**Check(s) violated:** Check 11 (Design coverage), Check 13 (Test tasks), Check 5 (Measurable criteria).

**Suggested fix:** Add a new feature `v03-benchmarks` (module 5) OR add GOALs to the relevant existing features:
- `GOAL-3.15` (retrieval): Benchmark suite reports LOCOMO overall ≥ 68.5%, LOCOMO temporal ≥ baseline, LongMemEval ≥ v0.2+15pp [P0]
- `GOAL-2.14` (resolution): Avg LLM calls per episode ≤ 3 over N=500 benchmark episodes, measured via write_stats [P0]
- `GOAL-4.9` (migration): All ~280 v0.2 tests pass after migration on real engram-memory.db [P0]
- Plus a retrieval-ranking regression GOAL for cognitive-feature effect.

Recommendation: **new benchmarks feature** — these criteria cut across resolution/retrieval/migration and deserve their own home with clear ownership. Would be 4-6 GOALs. Total would then be 5 features / ~55 GOALs, still manageable.

---

### FINDING-2 🔴 ✅ Applied — GOAL-4.9 added to v03-migration

**Location:** No feature has a GOAL for v0.2 API signature preservation.
**Issue:** DESIGN §7.3 explicitly promises:
- `store`, `recall`, `recall_recent`, `recall_with_associations` keep **unchanged signatures**
- These now route through v0.3 pipeline internally
- `store(content)` still works (graph extraction in background)
- `recall(query)` transparently does dual-level routing

This is a hard commitment to external consumers. GUARD-11 (master) talks about "deprecation shim for one minor version" but doesn't specify that the v0.2 call sites must compile unchanged.

The migration feature talks about database-level compatibility. Nothing talks about **crate API compatibility**.

**Check(s) violated:** Check 11 (Design coverage — §7.3 entirely uncovered), Check 27 (Missing paired-task: public API needs backward-compat task).

**Suggested fix:** Add to v03-migration (it's the natural home for "v0.2 → v0.3 transition"):
- `GOAL-4.9`: Existing v0.2 call sites (`store`, `recall`, `recall_recent`, `recall_with_associations`) compile against v0.3 without source changes and preserve their documented behavior. [P0] *(ref: DESIGN §7.3 Backward compatibility)*

Alternatively tighten GUARD-11 in master from [soft] to [hard] and reword to explicitly cover the 4 named methods.

---

### FINDING-3 🔴 ✅ Applied — GOAL-2.7 reworded to observable outcome (decision path inspectable, specific threshold count deferred to design)

**Location:** v03-resolution GOAL-2.7.
**Issue:** I pre-warned the sub-agent about threshold leakage, but on re-read the GOAL text names "three-path threshold routing" as the feature. "Three-path" + "threshold" is still a mechanism description — it prescribes the routing architecture. If implementation finds that a two-path scheme (auto-merge vs needs-review with human-loop) or a continuous-confidence scheme works better, GOAL-2.7 is violated even though the observable outcome (correct entity assignment) is met.

**Check(s) violated:** Check 8 (WHAT vs HOW — routing architecture is a design choice, not a requirement).

**Suggested fix:** Reword GOAL-2.7 to observable outcome:
- Before: "Candidates routed via three confidence bands: auto-merge, create-new, LLM-adjudicate"
- After: "For each candidate set, the fusion stage produces a final assignment (merge-into-existing-entity, create-new-entity, or defer-to-LLM) with confidence score attached. Callers can inspect why a given path was taken." [P0] *(ref: DESIGN §4.3 Stage 5)*

The "three paths" become observable (by inspection) without being required.

---

## 🟡 Important findings (should fix)

### FINDING-4 🔡 Dropped — review error

**Issue:** On re-read, GOAL-1.9 in v03-graph-layer does not contain the word "auditable". It says novel predicates do not participate in inverse/symmetric traversal — that is clear and testable. My review misread the doc. No change needed.

---

### FINDING-5 🟡 ✅ Applied — GOAL-2.11 kept as observability; new GOAL-2.14 added for runtime budget (≤4 rolling avg). Ship-gate ≤3 lives in v03-benchmarks GOAL-5.4.

**Location:** v03-resolution GOAL-2.11.
**Issue:** "LLM call count per episode is observable" — but the §11 ship gate says ≤3. GOAL-2.11 only requires observability; the budget enforcement is in soft GUARD-12 (warning only). A user reading just requirements would not know ≤3 is a ship condition. This ties into FINDING-1.
**Check violated:** Check 5 (Measurable), Check 27 (Missing paired task — observability without a target is half a requirement).
**Suggested fix:** Split into two GOALs: (a) observability (current wording, [P0]), (b) measured average ≤3 over rolling N≥100 window in production workloads [P1 — distinct from the ship-gate version in FINDING-1 which measures over benchmark suite].

---

### FINDING-6 🟡 ✅ Applied — GOAL-3.1 now requires ≥ 90% routing accuracy on ≥ 50-query labeled benchmark

**Location:** v03-retrieval GOAL-3.1 (query routing).
**Issue:** "Query routing is automatic" — how is this tested? The criterion "caller does not have to pick between graph and topic retrieval" is observable at the API level (single method, not two), but how do we test routing *correctness* — that "who is X married to" actually goes to graph and "what have I been working on" actually goes to topics? Without a correctness criterion, this GOAL passes trivially with a coin flip.
**Check violated:** Check 3 (Acceptance criteria), Check 5 (Measurable).
**Suggested fix:** Add: "Correctness of routing is validated on a labeled benchmark set of ≥50 queries (factual vs thematic); routing accuracy ≥90%." This gives implementation a concrete target and reviewers a test to run.

---

### FINDING-7 🟡 ✅ Applied — GOAL-3.8 now specifies Kendall-tau correlation < 0.9 under valence-delta ≥ 0.5 on ≥ 20-query set

**Location:** v03-retrieval GOAL-3.8.
**Issue:** "Same query under different mood states returns measurably different rankings" — measurably different by what metric? Kendall tau? Top-K overlap? Any difference at all (e.g., one swap in position 9 and 10)?
**Check violated:** Check 5.
**Suggested fix:** "…returns rankings with Kendall tau correlation < 0.9 when compared between two mood states differing by ≥0.5 on valence axis, on a fixed query set of size ≥20." Pick a concrete metric; implementation has a target.

---

### FINDING-8 🟡 ✅ Applied — GOAL-4.5 now specifies `(processed, total, succeeded, failed)` tuple, ≤ 100 records / ≤ 5s update cadence; ETA explicitly out of scope

**Location:** v03-migration GOAL-4.5.
**Issue:** "Migration progress is observable — caller can track how many records have been backfilled vs total, and can estimate completion." What update rate? Per-record? Per-100-records? For a 10M-record migration, per-record updates would drown the log.
**Check violated:** Check 5, Check 8 (HOW leakage — "estimate completion" requires an ETA algorithm).
**Suggested fix:** "Migration exposes a progress API returning (records_processed, records_total, records_failed) that updates at least every N records or every M seconds, whichever is first." Drop "estimate completion" — that's a UX nice-to-have, not a requirement.

---

### FINDING-9 🟡 ✅ Applied — GOAL-2.13 now notes promotion is governance activity (manual review) between versions; automatic promotion remains out of scope (ISS-031)

**Location:** v03-graph-layer GOAL-1.10 surfaces novel predicates; v03-retrieval GOAL-3.12 makes them queryable. Nothing covers **what happens next** — who reviews them, when they get promoted to canonical, who approves a breaking predicate-enum change.
**Check violated:** Check 27 (Missing paired task — new extensibility point needs a maintenance pathway).
**Suggested fix:** Either (a) acknowledge as deferred in master's Out of Scope ("Novel predicate promotion to canonical is manual / out of scope for v0.3.0"), OR (b) add a P2 GOAL: "A procedure exists to review accumulated novel predicates and promote frequent ones to canonical in a subsequent minor version." This is a governance requirement, not a code requirement.

---

### FINDING-10 🟡 ✅ Applied — GOAL-2.4 now specifies in-process concurrency only; cross-process explicitly out of scope (consistent with NG1)

**Location:** v03-resolution GOAL-2.4.
**Issue:** "Concurrent resolution of overlapping candidate sets does not corrupt the graph." This implicitly commits to concurrent writes. But DESIGN §1/NG1 says "distributed / multi-node deployment" is out of scope, and engramai today is single-node SQLite (one writer). Is GOAL-2.4 about:
- (a) In-process concurrent ingestion calls (multiple threads in the same agent), OR
- (b) Cross-process concurrency (two rustclaw instances hitting the same DB)?

The requirement doesn't say. (b) would be a big commitment — SQLite WAL mode handles it but the resolution pipeline's in-memory state wouldn't.
**Check violated:** Check 1 (Clarity).
**Suggested fix:** Clarify: "Concurrent in-process ingestion calls from multiple threads produce a consistent graph state (no partial writes, no duplicate entities created from racing resolves). Cross-process concurrency is out of scope — external callers must serialize access." Match this to DESIGN §1.

---

### FINDING-11 🟡 ✅ Applied — GOAL-4.6 resolved to preserve-plus-resynthesize; DESIGN §10 Q7 marked Resolved

**Location:** v03-migration GOAL-4.6.
**Issue:** "v0.2 topics are reconciled into L5: either carried forward with provenance, re-synthesized, or marked legacy" — three alternatives. In requirements-space, alternatives are bad: it means the decision hasn't been made, which means the acceptance criterion is "something happens with old topics" which is trivially true.

Per the spawn instruction, this was left deferred (Q7). But requirements shouldn't defer contract-level decisions — design can defer mechanism, requirements must fix outcome.
**Check violated:** Check 1 (Clarity), Check 3 (Testable).
**Suggested fix:** Pick one outcome now. My recommendation based on the design's thrust:
- "All v0.2 topics are carried forward into L5 with a `legacy=true` flag and provenance field pointing to their v0.2 source. Re-synthesis, if performed, runs in background and produces new topics alongside legacy ones; legacy topics are not deleted by re-synthesis."
- Mark Q7 in DESIGN §10 as resolved in the direction of "preserve-plus-resynthesize".

---

## 🟢 Minor findings (nice to fix)

### FINDING-12 🟢 ✅ Accepted — 14 GOALs in v03-retrieval is justified; no split

**Location:** v03-retrieval overall.
**Issue:** 14 > recommended soft cap of 12. Still under hard cap of 15. Reviewing shows no padding — each GOAL maps to a distinct design concern. Not splitting the feature.
**Suggested fix:** Accept. Note in summary that 14 is justified. If it grows past 15 in r2, consider splitting "mood-congruent + tiers" into a `v03-retrieval-affect` sub-feature.

---

### FINDING-13 🟢 ✅ Applied — GOAL-1.14 reworded to emphasize "derived, not stored as primary field"

**Location:** v03-graph-layer GOAL-1.14.
**Issue:** "Layer classification (Working/Core/Archived) is deterministic from existing fields." This is correct but could be stronger — the DESIGN §2 point is that layer is *derived*, not stored. Stating determinism is fine; stating non-storage is clearer.
**Suggested fix:** "Layer classification (Working/Core/Archived) is derivable from other fields on MemoryRecord and not stored as a primary field. Given the same record state, classification yields the same layer across processes and versions." Emphasizes non-storage.

---

### FINDING-14 🟢 ✅ Applied — master Open Questions section now lists Q1-Q10 with one-line summaries + affected features; Q7 marked Resolved

**Location:** `.gid/docs/requirements-v03.md` Open Questions section.
**Issue:** Section says "10 acknowledged open questions … see DESIGN-v0.3.md §10". Fine, but one-liner descriptions of each Q help a reader triage which ones might impact *their* feature without flipping to DESIGN.
**Suggested fix:** Add a bulleted list of Q1-Q10 with one-line summaries + which feature(s) they touch. ~15 lines added, saves a reader's round-trip.

---

## What was checked but passed

- **Numbering consistency**: All four features use their module prefix correctly. No gaps, no duplicates. ✅
- **GUARD referencing (not restating)**: Features list GUARD one-liners as reference index, not full restatements. ✅
- **Priority labels**: Every GOAL has [P0]/[P1]/[P2]. Distribution looks sane (heavy P0 in graph-layer, balanced elsewhere). ✅
- **GUARD severity**: Master has 10 hard + 2 soft. Hard guards enforce invariants, soft guards are cost/quality warnings. ✅
- **Naming convention**: GOAL-X.Y / GUARD-X only. No CR/INV/REQ/FR leakage. ✅
- **Refs to DESIGN**: All GOALs have `*(ref: DESIGN §X.Y)*` annotations pointing to specific subsections, not top-level sections. ✅
- **Out of Scope sections**: All 4 features + master have them. No contradictions between feature-level and master-level out-of-scope items. ✅
- **Module boundary rules (GUARD-4 through GUARD-6)**: Feature GOALs do not contradict these — cognitive state stays one-directional, empathy stays isolated, writes stay unblocked. ✅
- **Cross-feature references**: v03-retrieval references v03-graph-layer's L5 layer and v03-resolution's confidence scores — references are accurate. ✅

---

## Recommendation

**Apply the 3 🔴 findings before moving forward.** Specifically:
- FINDING-1 (ship-gate GOALs) — add a `v03-benchmarks` feature OR distribute the gates into existing features with [P0]
- FINDING-2 (v0.2 API backward compat GOAL) — add one GOAL to v03-migration
- FINDING-3 (GOAL-2.7 HOW-leakage reword) — 5-minute fix

The 🟡 findings can be fixed in the same pass or deferred to r2. Resolution of FINDING-11 (Q7 topic reconciliation) requires your decision on the preserve-plus-resynthesize direction.

The 🟢 findings are cleanup polish; fix when convenient.

After apply → proceed to `gid_design` for graph/task generation.
