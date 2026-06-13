---
id: ISS-225
title: 'Recall-ceiling umbrella: bi-encoder recall@200 ~0.51 is the true LoCoMo bottleneck (gold absent before reranking)'
status: open
priority: P1
severity: degradation
labels: [retrieval, recall, extraction, grounding, locomo, umbrella]
relates_to: [engram:ISS-201, engram:ISS-223, engram:ISS-141, engram:ISS-190, engram:ISS-191, engram:ISS-069]
filed: 2026-06-13
filed_by: rustclaw
---

## TL;DR

After exhausting every retrieval-side (ranking) lever under ISS-201,
three independent diagnostic lines converge on a single root cause:
**the gold memory is frequently not in the candidate pool *before*
reranking even begins.** This is a RECALL + GROUNDING problem, not a
ranking / K-sizing / fusion / MMR / prompt problem.

The measured ceiling: conv-26 bi-encoder **recall@200 = 0.507**
(single-hop 0.312, open-domain 0.000). Roughly half the gold evidence
is not in the top-200 candidates that any reranker, MMR pass, or
generation prompt ever gets to operate on. No amount of downstream
re-ordering can recover evidence that was never retrieved.

This issue is the umbrella tracking the recall/grounding work. It hangs
three concrete sub-directions off itself (A/B/C below) and records the
falsified ranking levers so nobody re-runs them.

## Why this issue exists (ISS-201 is closed)

ISS-201 closed the entire retrieval-side (ranking) surface. Every lever
was either falsified or proven corpus-overfit:

- **Hybrid-pool widening** — DEAD. The ~10-candidate cap people kept
  citing no longer exists: orchestrator.rs already sets the Hybrid RRF
  `top_k = query.limit.max(50)` (from prior lever-1). The `fuse_and_rank`
  bypass on the Hybrid arm is *by design* (RRF already fused the pool),
  and Hybrid candidates still flow through CE + MMR at Stage C.5. The
  10-candidate q51/q52 pools are **sub-plan starvation** (the plan only
  emitted 10), not a truncate cap. No Hybrid code fix exists.
- **KSEED (k_seed 10 → 250)** — FALSIFIED. Flat across all categories.
  The gold candidates are NOT sitting in ranks 11–250 waiting for the
  cross-encoder to see them. CE seed starvation is not the bottleneck.
- **Global K=30** — FALSIFIED as conv-26 overfit. conv-26 showed
  +7.9pp overall / +21.9pp single-hop / LIST net +5, but conv-44
  cross-validation was FLAT (8/8 churn). It only ever helped Problem-1
  scatter cases on one corpus.
- **Targeted list-intent K** — NOT WORTH BUILDING. Its only
  justification (dodge a conv-44 atomic regression) was bogus: the q48
  "regression" (gold = Family) was judge wobble (arm B dropped the word
  "family" from an otherwise-equivalent answer), and 9/16 conv-44 flips
  were temporal cross-ingestion noise. K=30 did nothing for conv-44 hard
  list zeros.
- **MMR-lambda sweep** — already done + FALSIFIED in ISS-223 (resolved
  2026-06-13): λ=0.7 conv-26 inert (0 flips, CE k_in=250 already
  reranked the head), λ=0.5 conv-26 +1.97pp but conv-44 −0.81pp (doesn't
  replicate). Default stays λ=1.0; λ=0.5 is list-heavy opt-in only.
  **Must `engram_recall` before proposing ANY bench experiment** so we
  never re-run a falsified lever.

**Process rule (hard):** before scoping or launching any experiment
under this umbrella, `engram_recall` the specific lever first. Two
levers in this investigation (MMR-λ, and the recall probe itself) were
nearly re-run from a stale premise; engram memory caught both.

## The three diagnostic lines that converged

1. **List scatter / absence.** LIST-bucket failures split into
   Problem-1 SCATTER (members present but spread across ranks 15–149,
   e.g. q15/q18/q24/q38) vs Problem-2 ABSENCE (members not in top-200
   at all, e.g. q34 0/2, q39 0/4, q19, q48, q32, q23). The absence half
   is pure recall failure.
2. **Temporal date-stranding.** Event memory is retrieved (rank 0) but
   its text carries NO resolved date, so temporal queries can't match
   and the generator honestly answers IDK. (Same root as conv-26-q0.)
3. **IDK-refusal.** On ISS222-LEVER2-conv26 (overall 0.5329) there are
   36 IDK-refusal failures (18 temporal). Cross-referencing the 18
   temporal-IDK against the recall probe: 13 gold are not in top-200 at
   all; the other 5 look "top-10" but are actually the **same entity,
   wrong specific memory** (q93 retrieved "bowl" not "necklace"; q94
   never retrieved "art and self-expression"). The generator's IDK in
   these cases is **honest and correct** — the gold memory genuinely is
   not in its context. The "fix the prompt" sub-bucket essentially does
   not exist.

All three reduce to: **recall + grounding.** Not ranking.

## Measured evidence (recall probe)

- Probe: `iss186_candidate_pool_probe`, conv-26, all 152 questions,
  top-k=200, pure bi-encoder cosine (bypasses fusion / CE / MMR).
- Artifact: `/tmp/recall_probe/conv26-all-20260613T034146Z.jsonl`
  (analyser `/tmp/recall_probe/analyse.py`).
- Result: aggregate recall@200 = **0.507**; single-hop 0.312;
  open-domain **0.000**. (open-domain 0.000 is itself a finding — those
  golds are simply not embedding-similar to their questions.)

## Sub-directions (each gets its own scoped issue once chosen)

### (A) Date-grounding — pin resolved dates into memory text/structured fields
- **Target:** the temporal date-stranding family (13/18 temporal-IDK
  failures where gold is not in top-200, plus conv-26-q0 / q106 / q118).
- **Prior art:** ISS-190 (duration → year) and ISS-191 (structured
  TemporalMark + interval temporal_score) already landed. The remaining
  gap is **event-relative → day**: "yesterday", "last Saturday",
  "two weeks ago" resolve to a precise day at extraction time but that
  day is stranded in a free-text `note` field while structured
  start/end collapse to a full-year interval, and the memory *text*
  carries no date at all.
- **Open question:** how much of the temporal recall gap is fixable by
  pinning the resolved day into both (a) the memory text the embedder
  sees and (b) the structured start/end interval, vs how much is
  genuinely ungroundable.

### (B) Query–document phrasing asymmetry — query expansion / HyDE bridging
- **Target:** the open-domain 0.000 recall and multi-hop event→preference
  mismatch (e.g. q19 dinosaur exhibit: the episode is in ep97 but ranks
  >200 because the question is phrased as a preference and the evidence
  is phrased as an event).
- **Prior art:** ISS-141 already contains the full HyDE / query-expansion
  design and layering decision (HyDE lives in the query layer, not
  retrieval; retrieval stays LLM-free/deterministic). This sub-direction
  is **revisiting/activating ISS-141**, not duplicating it.
- **Open question:** does HyDE/event→preference bridging actually pull
  the asymmetric golds into top-200, or is the embedding gap too wide
  even for a hypothetical-document rewrite?

### (C) Entity-memory disambiguation — right entity, wrong specific memory
- **Target:** q93 / q94 / q104 / q106 / q128 — the "top-10 but wrong
  memory" cases. The correct entity is recalled, but the wrong specific
  memory about that entity ranks above the gold one (q93 → "bowl"
  instead of "necklace").
- **Open question:** is the right fix at retrieval (better per-entity
  memory ranking / disambiguation signal) or at extraction (richer,
  more discriminative memory text so the gold memory embeds farther
  from its siblings)?

## Scope guardrails

- **Ungroundable gold must be excluded from any target metric.**
  Example: q34 "school speech" appears 0× across all 419 conv-26
  episodes; "mentor" appears only as "Caroline's supporters". There is
  no memory that *could* be recalled. Counting these against a recall
  target makes the metric un-achievable and hides real progress. The
  per-sub-direction issues must define an achievable-recall denominator
  that excludes ungroundable gold.

## Acceptance criteria (umbrella-level — closes when sub-issues are filed + scoped)

1. This umbrella issue is filed with the converged root cause and the
   falsified-lever record (done by filing).
2. Three sub-direction issues (A/B/C) are filed under this umbrella,
   each with: target query set, achievable-recall denominator (excluding
   ungroundable gold), a single concrete first experiment, and a
   falsification gate.
3. Sub-direction (A) date-grounding is scoped first (largest fixable
   temporal bucket).
4. No bench run is launched until the relevant lever has been
   `engram_recall`-checked against prior falsified work.

## Artifacts

- Recall probe: `/tmp/recall_probe/conv26-all-20260613T034146Z.jsonl`
  + `/tmp/recall_probe/analyse.py`
- ISS222-LEVER2 per-query (overall 0.5329, source of the 36 IDK
  bucket):
  `engram-bench/benchmarks/runs/ISS222-LEVER2-conv26-20260612T002433Z/2026-06-12T00-51-59Z_locomo/locomo_per_query.jsonl`
- conv-26 / conv-44 LISTK runs (K=30 falsification):
  `engram-bench/benchmarks/runs/ISS201-LISTK-{A,B}-conv26-20260613T060852Z/`,
  `engram-bench/benchmarks/runs/ISS201-LISTK44-{A,B}-conv44-20260613T073119Z/`
- ISS-201 verdict commits: e982a071, 3e9c2145, a983b5c9, bc3f3203,
  c5f7a630
