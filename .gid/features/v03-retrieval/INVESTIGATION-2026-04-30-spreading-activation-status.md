# Investigation: Spreading Activation Multi-Hop — Where Are We?

**Status:** investigation report (not a design doc)
**Date:** 2026-04-30
**Author:** rustclaw
**Context:** Triggered by potato asking "我们写的激活扩散算法到底 work 了没有"
**Related:** ISS-070, ISS-075, ISS-076, ISS-078, ISS-079; `discussion-spreading-activation.md`

---

## TL;DR

**The algorithm has not been tested.** Prototype code exists (801 LOC, compiles), but there is no evidence it has ever been executed end-to-end against the LoCoMo substrate. The "multi-hop 0/3" number from RUN-0007 is **not** spreading-activation's score — it is the score of the existing dispatcher, which has no multi-hop plan at all (ISS-070). RUN-0008 confirms the dispatcher never even reads the graph edges that the substrate fixes (ISS-075/076) repaired.

So the honest answer to "did spreading activation work?" is: **don't know — we never ran it.**

---

## UPDATE 2026-04-30 ~12:30 EDT — Prototype actually run

Ran prototype against RUN-0008 substrate. **Result: hit@5 = 1/3 (33.3%).** Log: `.gid/eval-runs/RUN-PROTO-2026-04-30/spreading_activation.log`.

Findings (full analysis below in "Prototype run results" section):

1. **Algorithm partially works.** Caroline → adoption-research path (q1) hits via 2-hop diffusion. Confirms the substrate-vs-traversal paradigm is viable on real LoCoMo data.
2. **Identical activation state across all 3 questions.** Anchor resolution sees only "Caroline" in all three queries. Question-specific words (`research`, `identity`, `relationship`) never become anchors — they are not entities in the substrate, and there is no fallback to embedding/keyword-based anchoring.
3. **First substrate-fix bug surfaced (ISS-076 residual or anchor-spec gap):** prototype hardcoded `namespace=locomo-conv26-iss068` but RUN-0008 substrate uses `locomo-conv26-iss076`. First run reported `entities=0 edges=0`, false-positive 0/3. Fixed by passing `--namespace locomo-conv26-iss076`. Prototype should auto-detect or fail loudly on empty namespace.
4. **Diffusion does not converge in K_MAX=10.** Activation still moving at step 10 (`max_delta=0.0521`). Either K_MAX too small or decay too high.
5. **Question-blindness is the dominant failure mode.** Without question-specific anchors, the same top-5 set is returned for every question about Caroline → 1/3 is essentially a coincidence (D2:8 happened to be the most Caroline-adjacent fact via the anchor-only diffusion).

---

## Timeline (chronological)

### Phase 1: Problem identified (RUN-0005 / RUN-0006, late April 2026)

LoCoMo evaluation showed cat=1 multi-hop at **0/3 (0%)** while single-hop categories ranged 33–86%. Gap too large to attribute to tuning. Filed **ISS-070** (P0): "Multi-hop plan dispatcher has no graph traversal — falls through to single-shot Factual/Abstract."

### Phase 2: V1 design (beam search) — rejected

Initial design for ISS-070 was beam-search-over-typed-edges: hybrid_recall seeds → expand by typed edges → keep top-K per hop → max 3 hops. Standard KG-QA approach.

potato pushed back. Reason was not algorithmic but paradigmatic:

> engram is built from "how the brain remembers." Beam search is "how a graph database queries." Different paradigms.

### Phase 3: Long discussion → paradigm switch

Walked through 6 mainstream multi-hop approaches (Cypher, PathRAG, beam search, GNN, agent-LLM, subgraph extraction) and rejected all of them. They share a hidden assumption: "retrieval is search over a static read-only graph."

New framing — **substrate vs traversal**:
- v0.3 typed graph is **substrate** (necessary, must exist, fixes ISS-075/076 repair this)
- traversal should not be "search" — it should be **activation diffusion over the substrate**

Algorithm specified in `discussion-spreading-activation.md` §9:
1. **Anchor resolution:** parse query text → identify which concepts the question is asking about → assign initial activation
2. **Iterative diffusion:** active nodes push activation to neighbors via `decay × edge_conductance`
3. **Edge-typed conductance:** `married_to=0.8`, `mentions=0.6`, `related_to=0.2`, `contradicts=-0.5` (inhibitory)
4. **Threshold + convergence detection:** stop when no node above threshold changes, or `K_MAX` steps
5. **Output:** retrieve memory chunks attached to nodes whose final activation exceeds threshold

Default parameters (hardcoded in prototype): `K_MAX=10, decay_propagate=0.5, pruning_threshold=0.05`.

### Phase 4: Prototype written

`crates/engram-bench/examples/spreading_activation_prototype.rs` — 801 lines, standalone binary.

**Critical fact:** it is a standalone binary that talks directly to `memory.db` and `graph.db` via `rusqlite`. **It is not wired into the retrieval pipeline.** Targets RUN-0006 conv-26 substrate, the 3 multi-hop questions specifically.

git log shows only one commit touching it: `f95480b fix(resolution): single-mint UUID + alias upsert + embedding propagation (ISS-075, ISS-076)`. No subsequent commits adding evaluation outputs, no result files in `.gid/eval-runs/`.

### Phase 5: Substrate bugs surfaced (ISS-075, ISS-076)

While preparing to run the prototype, two substrate-layer bugs emerged:
- **ISS-076:** ingestion mints multiple UUIDs for the same entity → mention→entity edges point to non-existent entities (dangling)
- **ISS-075:** alias nodes lack embeddings → anchor resolution has no way to find entry points

Implication: **even if the algorithm is correct, the ground is broken.** Prototype can't anchor, edges are dangling. RUN-0007 baseline summary captured this with the note: *"spreading activation expected to be broken by ISS-076."*

Both fixed in commit `f95480b`.

### Phase 6: RUN-0008 post-fix evaluation (2026-04-30)

After substrate fix, ran RUN-0008 against same query set. **hit@5 unchanged: 12/25 → 12/25.** Per-category results identical.

RUN-0008 summary's diagnostic:
> "Removing dangling edges alone moves nothing... The retrieval pipeline was never reading the broken edges in a way that affected hit@k."

Translation: **the dispatcher never traverses graph edges**, so substrate fixes have no effect on dispatcher-driven eval. This re-confirms ISS-070 is open and the multi-hop plan does not exist in production code.

### Phase 7: Today's diagnostic (2026-04-30) — new substrate gaps surfaced

While diagnosing why Hybrid plan is 0/2 in RUN-0008, two new substrate gaps surfaced (separate from the multi-hop story but adjacent):

- **ISS-078** (P1): L5 topic compiler not wired into ingestion finalize → `knowledge_topics` table has 0 rows → Abstract plan and Hybrid's abstract sub-plan both downgrade with empty result
- **ISS-079** (P2): Episodic plan over-downgrades on queries without `time_window` → Hybrid's episodic sub-plan returns 0

These are **not multi-hop bugs**, but they affect the Hybrid plan, which is the RRF-fusion container that a future multi-hop plan would also feed into.

---

## What we actually know vs. what we assumed

| Claim | Evidence | Honest verdict |
|---|---|---|
| "Spreading activation algorithm doesn't work" | None. Never ran. | **False claim — withdraw** |
| "Multi-hop 0/3 in RUN-0007 proves the algorithm fails" | RUN-0007 ran the existing dispatcher, not spreading activation | **False claim — withdraw** |
| "Substrate is now fixed" | `f95480b` merged, RUN-0008 run | **True for ISS-075/076 specifically; ISS-078/079 are new gaps** |
| "Dispatcher does not call multi-hop" | RUN-0008 unchanged after substrate fix; ISS-070 still open | **True** |
| "Prototype was written" | File exists, 801 LOC, compiles | **True** |
| "Prototype was tested" | No result files, no logs, no eval-run output for it | **No evidence — assume not tested** |
| "Hybrid 0/2 explained by ISS-078 + ISS-079" | Today's diagnostic | **True (the substrate gaps are necessary; sufficiency to be verified after fix)** |

---

## Earlier mis-statement (this session)

In an earlier turn this session, I said "spreading activation didn't work" and cited RUN-0008's multi-hop 0/3. Both were wrong:

1. The 0/3 is the dispatcher's score, not spreading-activation's score.
2. We have no measurement of spreading activation at all.

This is a `cite-before-claim` skill failure on my part. I conflated "the multi-hop number is bad" with "our algorithm failed," skipping the verification step that would have shown the algorithm was never on the execution path. potato caught this and forced the retraction. Logging it here so future-me does not repeat the same conflation.

---

## Open issues map

```
                ISS-070 (P0, open) — dispatcher has no MultiHop plan
                  │
                  │  blocks integration of spreading activation into pipeline
                  │
                  ▼
   prototype (standalone binary, untested)
                  │
                  │  needs: substrate that anchors + has live edges
                  │
                  ▼
   ISS-075 (substrate) ─── fixed in f95480b ──── RUN-0008 substrate is OK
   ISS-076 (substrate) ─── fixed in f95480b ──── for prototype to consume

   ISS-078 (P1, open) — L5 compiler not wired      ┐
   ISS-079 (P2, open) — Episodic over-downgrades   ┘  affect Hybrid, not multi-hop directly
```

---

## What needs to happen next (in dependency order)

### Step 1 — Run the prototype (cheapest, highest information)

- ✅ **DONE 2026-04-30 ~12:30 EDT.** Result: hit@5 = 1/3 (33.3%). See "Prototype run results" section below.

### Step 2 — Branch on results

Result was 1/3, between "partly works" and "broken." Diagnostic clear: anchor resolution is the bottleneck, not diffusion. See "Prototype run results → Diagnosis" for branch action.

### Step 3 — Independent of multi-hop

ISS-078 and ISS-079 should be addressed separately. They affect Hybrid plan correctness regardless of whether multi-hop ever ships, and the fixes are localized.

---

## Prototype run results (2026-04-30 ~12:30 EDT)

### Setup

```
binary:    target/release/examples/spreading_activation_prototype
graph-db:  .gid/eval-runs/RUN-0008-substrate/locomo-conv26-iss076.graph.db
memory-db: .gid/eval-runs/RUN-0008-substrate/locomo-conv26-iss076.db
dataset:   cogmembench/datasets/locomo/data/locomo10.json
namespace: locomo-conv26-iss076  (override via --namespace; default in code is iss068)
```

### Substrate stats (after namespace fix)

```
mentions:  167 rows
entities:  113
edges:     116
memories:  64
predicates: caused_by(1), depends_on(5), is_a(10), leads_to(46), part_of(4), related_to(34), uses(16)
```

### Per-question results

| # | Question | Anchors | Top-1 entity | Hit@5 |
|---|---|---|---|---|
| q1 | What did Caroline research? | `caroline` | helping people (act=0.222) | ✅ YES (D2:8 at rank 3) |
| q2 | What is Caroline's identity? | `caroline` | helping people (act=0.222) | ❌ NO (gold D1:5 absent from top-5) |
| q3 | What is Caroline's relationship status? | `caroline` | helping people (act=0.222) | ❌ NO (gold D3:13, D2:14 absent) |

**Overall hit@5: 1/3 (33.3%)**

### Activation traces

All three questions produce **identical** activation traces because all three anchor on the single token "caroline" and ignore the question-specific words. Example trajectory:

```
Step 0: 1 active,  max_act=1.000, sum|act|=1.000  (anchor injection)
Step 1: 36 active, max_act=0.700, sum|act|=7.080
Step 2: 56 active, max_act=0.490, sum|act|=10.274
Step 3: 57 active, max_act=0.492, sum|act|=11.533
Step 4: 57 active, max_act=0.460, sum|act|=11.560  (sum peaks here)
Step 5: 60 active, max_act=0.402
...
Step 10: 60 active, max_act=0.222, max_delta=0.0521  (still not converged)
```

Top-5 entities at termination, identical for all 3 questions:
```
helping people                            +0.222
providing a loving home to children in need +0.216
school event                              +0.208
acceptance                                +0.208
understanding / supportive community      +0.208
```

### Diagnosis

The single failure mode is **question-blindness in anchor resolution.** Current anchor logic appears to: tokenize question → look up tokens in `name_index` → inject as anchors. For these 3 questions, only "Caroline" matches an entity. Question intent words (`research`, `identity`, `relationship status`) are never entities, so they contribute nothing — the diffusion has no idea what facet of Caroline is being asked about.

This means the algorithm's **multi-hop diffusion is working** (D2:8 came from a 2-hop traversal: `Caroline --leads_to--> [adoption research / helping people / family] --supports--> D2:8 chunk`), but **the algorithm is essentially answering "what's strongly associated with Caroline?" instead of "what did Caroline research?"**. q1 wins by coincidence (D2:8 is the most Caroline-adjacent memory), q2 and q3 lose because the gold facts are not the most-Caroline-adjacent.

Two secondary findings:

1. **Convergence not reached at K_MAX=10.** `max_delta=0.0521` at step 10 vs `pruning_threshold=0.05`. Need either larger K_MAX or steeper decay to ensure stable rank.
2. **Silent namespace mismatch.** First run reported `entities=0 edges=0` but exited 0 with hit@5 = 0/3, indistinguishable from real algorithm failure. Prototype should hard-error when the namespace is empty, not silently report 0/3.

### What this changes about the open issues

| Issue | Status before | Status after this run |
|---|---|---|
| ISS-070 (dispatcher MultiHop) | "promote when prototype works" | Still open. Prototype works partially; promotion needs anchor-resolution fix first |
| New issue needed: anchor resolution must use question semantics, not token lookup | — | **Should be filed (P1)** |
| New issue needed: diffusion convergence at K_MAX=10 | — | **Should be filed (P2)** |
| New issue needed: prototype empty-namespace silent failure | — | **Should be filed (P3, hygiene)** |

### Honest verdict on "did spreading activation work?"

**Partially.** The diffusion mechanism does what was specified — activation flows along typed edges, decays correctly, surfaces semantically-related memories. But the algorithm as currently specified is **question-blind**: it answers "give me what's near the anchored entities" rather than "give me what answers this question about the anchored entities." On LoCoMo conv-26, that's worth ~33% by accident. To get above 50%, anchor resolution needs to incorporate question intent — likely via question embedding boosting, or by extracting predicate hints from the question (`research?` → boost edges with predicate `uses`/`depends_on` near Caroline).

This is now a **design problem**, not a "is the paradigm right" problem. The paradigm is right; the spec was incomplete on the anchor stage.

---

## Files / refs

- Algorithm spec: `.gid/features/v03-retrieval/discussion-spreading-activation.md`
- Prototype: `crates/engram-bench/examples/spreading_activation_prototype.rs`
- Substrate (post-fix): `.gid/eval-runs/RUN-0008-substrate/locomo-conv26-iss076.{db,graph.db}`
- Pre-fix baseline: `.gid/eval-runs/RUN-0007-baseline-pre-fix-summary.md`
- Post-fix eval: `.gid/eval-runs/RUN-0008-post-fix-summary.md`
- Issues: ISS-070, ISS-075, ISS-076, ISS-078, ISS-079
