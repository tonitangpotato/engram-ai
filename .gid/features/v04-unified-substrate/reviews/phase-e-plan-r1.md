# Review: PHASE-E-PLAN.md (r1)

Reviewer: RustClaw (self-review, review-design methodology)
Date: 2026-05-31
Target: `.gid/features/v04-unified-substrate/PHASE-E-PLAN.md` @ commit a05cf66
Verdict: **3 Critical, 2 Important, 1 Minor — plan needs revision before T34a**

The plan's structure, rollback strategy, and per-cluster protocol are sound.
The defects are all in **scope accuracy (§3/§4)** — the legacy-write counts and
cluster boundaries don't match ground truth, which risks silently leaving live
legacy writes after the "exit gate" passes.

---

## 🔴 FINDING-1 (Critical) — graph/store.rs write count is 8, not 3

§3 and §4 (T37) say graph/store.rs has "3 resolution-pipeline edge writes".
Ground truth (grep): **8 `INSERT INTO graph_entities|graph_edges`** spread across
multiple methods (insert_entity ~4874, insert_edge ~5439/5650, merge_entities
~6450/6577, plus the dual_write_edge_to_edges sites ~5338/5500/6635).

Risk: T37 scoped to "3" would delete ~3 and the T37x exit gate (if it only re-greps
the same wrong pattern) could pass with ~5 live legacy writes remaining.

Fix: re-inventory graph/store.rs by method. T37 must enumerate all 8 by line+method,
and distinguish: which are the *legacy* writes to delete vs. the *unified* dual-write
helper calls (dual_write_edge_to_edges / dual_write_entity_to_nodes) that STAY.
graph/store.rs is BOTH a trait def and impl — be precise about which fn each lives in.

## 🔴 FINDING-2 (Critical) — storage.rs count is 69, not 78; distribution unstated

§3 says "78 legacy writes". Ground truth: ~69 across memories(5) + memories_fts(7)
+ hebbian_links(12) + memory_entities(3) + synthesis_provenance(2) +
memory_embeddings(2) + triples(1) + UPDATE memories(16) + DELETE legacy(22).

The "78" appears to be a stale inventory number from design §5.5. Without an
accurate per-table breakdown, cluster boundaries (§4) can't be validated for
completeness — there's no checklist to confirm "all N deleted".

Fix: replace §3 with the verified per-table distribution above. Each cluster in §4
must list the exact line numbers it owns, and the sum across clusters must equal
the total. T37x exit gate verifies the sum is 0.

## 🔴 FINDING-3 (Critical) — `triples` write is unassigned to any cluster

storage.rs:7296 `INSERT OR IGNORE INTO triples` exists. design §7.6 confirms
`triples` is in the DROP set (0 rows, no reader). So its write MUST be removed in
Phase E — but §4's clusters (T34a–T37) never mention triples. It would survive to
T39, where DROP triples + a live INSERT INTO triples = the exact ISS-196-class
collision this whole exercise is meant to prevent.

NOTE: design §5.5.3 / T26a already flagged `insert_triple_entity` (store_triples)
writes entities+memory_entities via raw SQL that does NOT cascade through the
ISS-123 dual-write — i.e. triple-path has known dual-write debt. The triples write
deletion may be entangled with that gap. Verify before assigning a cluster.

Fix: add T36b cluster for the triples write, OR confirm it's covered by an existing
issue and explicitly defer with a tracking ref. Do not leave it implicit.

## 🟡 FINDING-4 (Important) — UPDATE/DELETE families under-specified

§4 T34c (UPDATE family) and T34d (DELETE family) name only 3 + 2 entry points,
but ground truth shows 16 UPDATE-memories and 22 DELETE-legacy statements. Many
are in update_content/update_importance/delete_embedding/hard-delete (already
dual-writing per ISS-124/125/126), but the count gap means some UPDATE/DELETE
sites aren't accounted for.

Fix: enumerate all 16 UPDATE + 22 DELETE sites, map each to a cluster, confirm
each already has a unified-side counterpart (else it's not a safe deletion — it's
a behavior change).

## 🟡 FINDING-5 (Important) — no explicit "unified survivor exists" check per write

§5 step-1 says "verify the unified survivor exists and fires" but the plan never
records, per legacy write, WHAT the survivor is. For add() it's
insert_memory_node_row. For hebbian it's record_coactivation→edges. For UPDATE
it's the ISS-124 dual-update. Without a written map, a deletion could remove a
legacy write that has NO unified counterpart yet → silent data-write loss.

Fix: add a §3.1 table: [legacy write site] → [unified survivor] → [verified? Y/N].
A deletion is only safe when survivor=verified. Any "N" row blocks that cluster.

## 🟢 FINDING-6 (Minor) — T37x exit gate method unspecified

§4 T37x says "AST-grep audit proving 0 prod legacy writes" but doesn't pin the
exact pattern/tool. Given FINDING-1/2 (grep patterns were themselves wrong), the
exit gate's reliability depends entirely on the pattern being complete.

Fix: specify the exit-gate as: grep ALL of {INTO|UPDATE|DELETE FROM} × {memories,
memories_fts, hebbian_links, memory_entities, synthesis_provenance,
memory_embeddings, memory_embeddings_v2, graph_entities, graph_edges, triples} in
src/ excluding test modules + migration DDL, expect 0 matches.

---

## What's GOOD (keep as-is)

- Three-layer rollback (tag / per-cluster commit / data-immunity) is correct and
  the data-immunity insight (Phase E deletes code not data) is the right framing.
- Per-cluster "test green before next cluster" protocol is exactly right.
- Stop conditions §6, esp. "NEVER touch T39 autonomously", are correct.
- Lowest-risk-first ordering is sound; T35 Hebbian flagged as high-risk + surface
  point is good judgment.
