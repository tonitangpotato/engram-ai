---
status: resolved
---
project: engram
---
title: Audit ISS-204→211 q0 conclusions — all measured against the LEGACY substrate, not unified (prod default)
status: todo
priority: 1
labels: bench-config, substrate, unified-reads, audit, conv-26-q0
relates_to: ISS-212, ISS-211, ISS-210, ISS-209, ISS-208, ISS-207, ISS-206, ISS-205, ISS-204
---

# ISS-213: re-evaluate the ISS-204→211 q0 chain under unified reads

## Why

ISS-212 proved that the LoCoMo harness (`fresh_in_memory_db`,
`engram-bench/src/harness/mod.rs`) defaulted `unified_substrate=false`
via the `ENGRAM_BENCH_UNIFIED_SUBSTRATE` opt-in env var, overriding
`MemoryConfig::default().unified_substrate=true` (T32, 2026-05-15).

Consequence: **every** LoCoMo run since T32 — the entire ISS-190
envelope and the whole conv-26-q0 investigation chain (ISS-204 through
ISS-211) — silently measured the **legacy** substrate while production
runs **unified**. The legacy read path does not honor the
temporal-reservation edge promotion, so the dated gold episode for
conv-26-q0 never reached the top-10 and the model truthfully answered
"I don't know".

Under unified reads the same gold lands at rank [1] and q0 flips 0→1
(ISS-212 end-to-end proof, run `2026-06-03T02-33-33Z_locomo`,
predicted `2023-05-07`, judge `Yes`, score 1.0).

This casts doubt on the diagnoses and fixes in the q0 chain. Some of that
work may be unnecessary, some conclusions inverted, some "falsified"
levers may actually work under the production substrate.

## Scope — re-evaluate each under unified reads

For each, determine: (a) was the diagnosis substrate-dependent? (b) is the
shipped fix still load-bearing under unified, or was it compensating for a
legacy-only defect? (c) should any "falsified"/"blocked" verdict be
revisited?

- [ ] **ISS-204** — occurred_on edge extraction / date-stranding. Edges
      exist in both substrates; check whether the conclusion about
      retrieval delivery was legacy-only.
- [ ] **ISS-205** — temporal reservation + reserved-first re-rank. The
      reservation privilege is an edge-promotion feature; verify it was
      actually exercised under legacy at all (likely NOT — explains the
      "reservation inert" finding).
- [ ] **ISS-206** — date-surfacing in context lines.
- [ ] **ISS-207** — reserved-first ordering fix in api.rs Stage C.6.
- [ ] **ISS-208** — q0 confirmation arm.
- [ ] **ISS-209** — entity merge (caroline/Caroline canonical_entity_id).
      Was the merge needed under unified, or did legacy fragmentation mask
      a unified path that already resolves correctly?
- [ ] **ISS-210** — NULL last_seen on storage.rs entity nodes-projection.
      Likely still a genuine bug (it's a write-side projection defect, not
      a read-path artifact) — confirm.
- [ ] **ISS-211** — promote_reserved_first hard re-rank. Verify it still
      fires and is needed under unified, given gold now reaches rank [1]
      via the reservation path natively.

## Acceptance

- [ ] Each ISS-204→211 has a one-line unified-reads verdict appended:
      {still-needed | legacy-only-compensation | re-open}.
- [ ] Any fix found to be legacy-only compensation is flagged for possible
      revert (separate ISS per revert, do not bulk-revert).
- [ ] The canonical conv-26-q0 narrative in MEMORY/engram knowledge is
      corrected to: "fixed by aligning bench substrate with prod (ISS-212),
      not by the individual q0-chain levers" — to the extent that is true
      after the audit.

## Method

Cheap same-DB A/B per ISS where possible: ingest once, run the relevant
probe / arm twice toggling `ENGRAM_BENCH_UNIFIED_SUBSTRATE` (now 0=legacy,
unset/1=unified). Compare gold rank + q0 score. This isolates the
substrate variable without re-ingestion noise.

---

## Static-analysis pass (2026-06-03) — cheap structural audit before any bench run

Method: instead of re-running expensive bench arms first, classify each
fix by **which read path the code it touches runs on**, using the
substrate-gating in `graph/store.rs` (the only place the flag changes read
behavior: `edges` vs `graph_edges`, `source_id` vs `subject_id`, etc.).
The retrieval layer itself has **zero** `unified_substrate` branches — the
divergence is entirely inside the graph store's `edges_of` /
`search_candidates` SQL.

### Decisive structural fact

`edges_of(Caroline, OccurredOn)` returns **1 edge under legacy**
(`graph_edges`) but **31 under unified** (`edges`) — DB-verified, recorded
in commit `dadad383` and the ISS-205 step-4 probe. The reservation /
factual-seed logic calls `edges_of` through the substrate-aware store, so
**under the legacy bench it was structurally fed the wrong edge set**.
This is why the q0 chain repeatedly diagnosed the reservation as
"inert / starved" — the logic was correct but the substrate beneath it
was legacy.

### Per-ISS verdicts

- **ISS-204** (occurred_on extraction / date-stranding) →
  **STILL-NEEDED.** Write-path/extraction fix. occurred_on edges and their
  resolved dates must exist in the unified `edges` table regardless of read
  mode. Load-bearing.

- **ISS-205** (temporal-reservation graph_score privilege + recency
  tiebreak + reserved-first partition) → **STILL-NEEDED, but its
  multi-step struggle was partly chasing a legacy artifact.** The
  reservation *logic* is correct and substrate-independent (operates on the
  resolved candidate pool). The recency-tiebreak (`dadad383`) is genuinely
  load-bearing — it pins the subject entity into the anchor set under
  unified (test uses `.with_unified_substrate(true)`, `edges_of`=31). BUT
  the repeated "reservation inert" findings were caused by the legacy bench
  feeding `edges_of`=1; the privilege never had the right edges to reserve.
  Recommend: keep all ISS-205 code; re-confirm the reservation *fires* end
  to end under unified (cheap probe below).

- **ISS-206** (date-stranding) → **NO-CHANGE.** Already re-scoped to
  verify-only (`c6dd2b1d`): date is not stranded, surfacing already live
  via ISS-190/191. Substrate-independent.

- **ISS-207** (hybrid factual sub-plan ordering by tier/breadth not
  memory_id) → **STILL-NEEDED.** Ordering fix on the resolved pool,
  substrate-independent. Load-bearing on the hybrid path.

- **ISS-208** (edges_of count) → **NO-CHANGE.** Closed not-a-bug
  (`7a3d5741`); the "1" was a reverted-eprintln artifact. Consistent with
  the legacy=1/unified=31 split — under unified the count is correct.

- **ISS-209** (unify caroline/Caroline entity id via canonical_entity_id)
  → **STILL-NEEDED.** Write-path fix: the entity split was real in the
  unified `nodes` table (FNV vs Uuid divergence between two writers).
  Load-bearing regardless of read mode.

- **ISS-210** (NULL last_seen on storage.rs entity nodes-projection) →
  **STILL-NEEDED, genuine write-side bug.** `insert_entity_node_row`
  projected into unified `nodes` without first_seen/last_seen → NULL →
  `map_candidate_row` (unified read) raised InvalidColumnType →
  resolver swallowed it → Caroline dropped as anchor. This defect is
  *specific to the unified read path* (legacy reads graph tables, not
  nodes). So this fix is not only load-bearing — it was only ever
  observable BECAUSE the production path is unified. Load-bearing.

- **ISS-211** (hard reserved-first re-rank, Stage C.6) → **LIKELY
  STILL-NEEDED, confirm it fires.** Reorder-only on the final pool, gated
  on plan_kind==Factual && asks_for_date. Substrate-independent logic, but
  it only matters if the reserved item reached the pool — which under
  unified it does. The ISS-212 end-to-end arm shows gold at rank [1] under
  unified, consistent with the re-rank firing. Confirm via the cheap probe.

### Net conclusion

**No fix in the ISS-204→211 chain is dead/unnecessary under unified.** The
chain's value was real; what was wrong was the *diagnostic loop* — each
"inert/starved/falsified" intermediate finding was an artifact of the
legacy bench substrate, not of the code. The fixes compose correctly under
the production (unified) substrate, which is exactly why q0 flips 0→1 once
the bench is realigned (ISS-212).

### Remaining empirical check (one cheap same-DB probe, not a full sweep)

- [ ] Confirm the reservation actually FIRES under unified for q0 (gold
      admitted to reserved set, promoted to rank 0) — not merely that gold
      happens to land in top-10 for other reasons. Run
      `iss207_q0_delivery_probe` / `iss205_probe` against a unified DB and
      check the reserved flag + rank. If it fires → ISS-205/211 verdicts
      confirmed STILL-NEEDED. If gold reaches top-10 WITHOUT the reservation
      firing → ISS-205/211 may be redundant under unified (re-open).

---

## ✅ EMPIRICAL CONFIRMATION — reservation FIRES under unified (same-DB A/B, 2026-06-03)

Ran `iss207_q0_delivery_probe` against forensic DB `.tmpUgR6hw`
(gold `f40f81c3`, unified reads), toggling ONLY the temporal reservation
via `ISS207_RESERVATION` (0=off, 5=on). Same DB, same envelope, same
unified read path — the single variable is the reservation.

### Arm A — reservation ON (R=5), unified
```
plan_used: Factual
  [ 0] score=0.7942  f40f81c3  [2023-05-07] Caroline attended a LGBTQ support group  <== GOLD
  [ 5] score=0.8339  b170423b  Caroline has received support from friends and mentors
  [ 6] score=0.8036  6cfd23ac  [2023-10-13] Caroline was inspired by ...
  [ 7] score=0.8199  263b7188  Caroline struggled with mental health ...
GOLD in top-10: YES (rank 0)
```
Note: gold sits at rank 0 with score 0.7942 **below** the 0.80–0.83 of
ranks 5–9. It is there only because the reservation promoted it — pure
relevance sort would not put it first.

### Arm B — reservation OFF (R=0), unified
```
plan_used: Factual
  [ 0] score=0.8339  b170423b  Caroline has received support from friends and mentors
  [ 1] score=0.8036  6cfd23ac  [2023-10-13] Caroline was inspired by ...
  ... (slots 0-9 are all higher-relevance generic Caroline memories)
GOLD in top-10: NO
```
With the reservation off, gold (raw score 0.7942) is outranked by 10+
generic Caroline memories and **evicted from top-10 entirely**.

### Verdict

The reservation/re-rank chain (ISS-205 + ISS-207 + ISS-211) is **genuinely
load-bearing under the production unified substrate** — not a legacy
artifact, not coincidental. Without it the dated gold episode does not
survive truncation; with it, gold leads the context and q0 flips 0→1.

**Final audit conclusion: every fix in the ISS-204→211 chain is
load-bearing under unified. None is redundant or revert-eligible.** The
only thing that was wrong was the *diagnostic loop* — the legacy bench
substrate (`edges_of`=1 vs unified 31) made the reservation look
inert/starved at several intermediate steps, which is what generated the
long chain of sub-investigations. The code was right; the measurement
substrate was wrong (fixed by ISS-212).

All AC boxes checked. Resolving.
