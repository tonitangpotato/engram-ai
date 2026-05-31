# Review: PHASE-E-PLAN.md (r2)

Reviewer: RustClaw (self-review round 2, review-design methodology)
Date: 2026-05-31
Target: `.gid/features/v04-unified-substrate/PHASE-E-PLAN.md` @ r2 (uncommitted)
Prior: `reviews/phase-e-plan-r1.md` (3 Critical, 2 Important, 1 Minor)
Verdict: **r1 findings all resolved. 1 new Important, 1 Minor. Plan is executable
after addressing FINDING-7.**

---

## r1 finding disposition

- **FINDING-1 (graph/store.rs count)** ✅ RESOLVED. r2 §3.0 states 6 prod INSERTs
  with exact fn+line (insert_entity@4874, merge_entities@5338, insert_edge@5439,
  supersede_edge@5650, apply_graph_delta@6450+6577), and explicitly separates the
  3 dual_write_* survivor calls (KEEP). Verified against awk fn-mapping.
- **FINDING-2 (storage.rs count + distribution)** ✅ RESOLVED. r2 §3.0 gives full
  per-table prod breakdown summing to 57; arithmetic re-verified (57+6=63). The
  test-boundary exclusion (L8741) is the root cause of the old inflated counts —
  correctly identified.
- **FINDING-3 (triples unassigned)** ✅ RESOLVED. r2 adds T36b for the triples
  write with the entanglement caveat (T26a dual-write debt) and a defer-with-ref
  escape. §3.1 marks it "delete outright, no survivor".
- **FINDING-4 (UPDATE/DELETE under-specified)** ✅ RESOLVED. r2 T34c owns all 14
  UPDATE + superseded_by; T34d owns 2+6 deletes; T36a owns the entity/embedding/
  synth deletes. Counts now explicit per cluster.
- **FINDING-5 (no survivor map)** ✅ RESOLVED. r2 §3.1 is the survivor table with
  a Verified column; protocol §5 rule "UNVERIFIED ⬜ = not deleted" enforces it.
- **FINDING-6 (exit-gate method)** ✅ RESOLVED. r2 §6.1 pins the exact grep
  pattern + the 63→0 cluster-by-cluster cross-check.

All six prior findings are genuinely fixed, not papered over.

---

## 🟡 FINDING-7 (Important, NEW) — cluster sum ≠ 63; T34a/b overlap unaccounted

§4 clusters don't visibly sum to 63, and there's a double-count risk: §3.0 counts
`memories INSERT: 3`, but T34a claims "memories INSERT + memories_fts + FTS-rowid
SELECT (3 stmts)" while T34b also claims "store_raw legacy memories/FTS writes".
One of the 3 `memories` INSERTs is in add(), one in store_raw(), one in
update_content (rebuild path ~2832). If T34a says "3 stmts" but only owns add()'s
slice, the cluster-owns-N accounting is ambiguous.

Risk: at T37x, "63 → 0" can't be checked cluster-by-cluster if individual clusters
don't declare exact line numbers they own (only categories). The §6.1 grand-total
check still works as a backstop, but the per-cluster cross-check (§4 last line,
§6.1) is not actually executable as written.

Fix: either (a) annotate each cluster with the exact line numbers it owns (so the
union = the §3.0 line set), or (b) downgrade the per-cluster "sum = 63" claim to
"the §6.1 grand-total grep is the authoritative completeness check; clusters are
organizational, not arithmetic partitions." Option (b) is lighter and honest —
line numbers shift as deletions happen, so a fixed per-cluster line map goes stale
mid-execution anyway. Recommend (b).

## 🟢 FINDING-8 (Minor, NEW) — memory_embeddings_v2 survivor unconfirmed in design

§3.1 maps both memory_embeddings and memory_embeddings_v2 → "node_embeddings table"
with ⬜. But the review couldn't confirm from design that node_embeddings is the
canonical unified embedding store (the design grep for memory_embeddings_v2
returned nothing). If node_embeddings is NOT the unified survivor, deleting the
_v2 write loses embeddings silently.

Fix: T36a step-1 must explicitly confirm node_embeddings is written by a unified
path AND read by the unified retrieval path before deleting either embedding write.
This is already implied by the §5 protocol, but given _v2 has no design hit, call
it out as a named stop-condition for T36a. (Low severity — protocol already gates
it; this just raises visibility.)

---

## What's GOOD (unchanged from r1, still holds)

- Three-layer rollback + data-immunity framing: correct.
- Per-cluster green-test protocol + "no two clusters without green between": correct.
- §3.1 survivor map with Verified gate is the single most important addition — it
  converts "delete and hope" into "delete only when counterpart proven".
- Lowest-risk-first ordering; T35 Hebbian + T36b triples flagged as surface points.
- §6.1 explicit exit gate closes the FINDING-6 reliability hole.

---

## Recommendation

Apply FINDING-7 option (b) (one-line clarification that §6.1 grand-total is the
authoritative completeness check). FINDING-8 is already protocol-gated; adding the
named T36a stop-condition is a 1-line improvement. After these, the plan is
execution-ready and T34a can begin.
