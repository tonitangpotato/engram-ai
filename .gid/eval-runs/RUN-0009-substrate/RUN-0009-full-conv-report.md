# RUN-0009: Full conv-26 substrate retrieval report

**Date**: 2026-04-30
**Binary**: post ISS-075 + ISS-076 fixes (same as RUN-0008)
**Substrate**: `locomo-conv26-full` — 19 sessions, 419 turns, 0 ingest failures
**QAs**: all 199 from locomo-conv26 (cat 1-5)
**Method**: substrate-only retrieve, **NO LLM-as-judge** (see §1)

---

## TL;DR

- Headline **hit@5 (cat 1-4) = 79/150 = 52.7%** — vs 50% on RUN-0007/0008 3-session sample. ~3pp lift on 7.5× more QAs.
- This is **substrate quality (recall@5 of gold dia_id), not answer accuracy.** No model is asked to answer.
- Two known holes are now quantified at scale:
  - **Hybrid plan: 0/10 hits, all `empty_result_set`.** Root cause located (see §3).
  - **Multi-hop (cat=1): 10/33 = 30.3% hit, and 0 of them dispatch through Hybrid.** All 32 multi-hop questions land on Factual or Abstract — same fall-through pattern ISS-070 documents.
- Spreading-activation prototype was tried 2026-04-30 and got **1/3 in-window hits** — the 1 hit was a coincidence (anchor resolution is question-blind; activation pattern is identical for q1/q2/q3). Wiring the current prototype into `RetrievalEngine` would not move cat=1. See §4.

---

## 1. Why this finished in ~2 minutes — there is no judge

The retrieve script runs `crates/engramai/examples/locomo_conv26_retrieval.rs`. It is a **substrate-level recall test**:

```
for each QA in locomo10.json:
    candidates = retrieval_engine.retrieve(query, ns=locomo-conv26-full, k=5)
    hit = any(c.source contains gold_dia_id for c in candidates)
```

- No answer is generated.
- No LLM is called for judging.
- No ROUGE / F1 / exact-match against the gold answer string.
- Only metric: did any of the top-5 retrieved memories carry a `source` field matching the question's gold `dia_id`?

199 queries × ~600ms each ≈ 2 minutes. Same speed as RUN-0007/0008 per-query; just more QAs.

### Implication for cross-system comparison — we cannot publish a J score yet

The numbers in this report are **internal substrate diagnostics**, not comparable to published LoCoMo results from Mem0 / MemGPT / LangGraph / Letta / etc. Those papers report **J score** (LLM-judged answer correctness, the LoCoMo paper's headline metric). Our 52.7% is recall@5 of the gold dialog; it tells us whether the right evidence is in the candidate set, not whether a downstream model can produce the right answer from it.

Two things are true:
- **Recall@5 is upstream of J.** If recall@5 is bad, J cannot be good — the model can't answer from evidence it never sees. So this number is still a useful internal floor.
- **High recall@5 does not imply high J.** A reader model still has to (a) parse the right span out of 5 candidates, and (b) reason correctly. Multi-hop especially loses points at (b).

To compare with other systems we need an LLM-as-judge track. Sketch:
1. For each QA, after substrate retrieval, pass top-5 candidates + question to an answer model (gpt-4o or similar — match the comparison paper's choice).
2. Score the generated answer against gold using LoCoMo's published judge prompt (or `cogmembench`'s judge).
3. Report J alongside recall@5.

Cost on full 199 QAs at ~$0.01/call × 2 calls (answer + judge) = ~$4 per run. Cheap. **Reason it's not in this report**: the retrieve binary doesn't yet plug into a judge — `cogmembench` has the judge code on its side, but we haven't wired the engram retriever as a `cogmembench` adapter for conv-26 yet. Tracked as §6 follow-up.

---

## 2. Aggregate numbers

### Headline (cat 1-4 only, the published LoCoMo metric)

| Run | Sessions | QAs (cat1-4) | hit@5 |
|---|---|---|---|
| RUN-0007 (pre-fix) | 1-3 | 20 | 50.0% (10/20) |
| RUN-0008 (post-fix) | 1-3 | 20 | 50.0% (10/20) |
| **RUN-0009 (full)** | **1-19** | **150** | **52.7% (79/150)** |

### All 199 QAs by category

Categories (LoCoMo taxonomy):
- **cat=1** multi-hop reasoning
- **cat=2** temporal reasoning
- **cat=3** open-domain
- **cat=4** single-hop
- **cat=5** adversarial / unanswerable

| Category | n | hits | hit@5 |
|---|---|---|---|
| cat=1 multi-hop | 33 | 10 | **30.3%** |
| cat=2 temporal | 38 | 33 | 86.8% |
| cat=3 open-domain | 12 | 4 | 33.3% |
| cat=4 single-hop | 71 | 32 | 45.1% |
| cat=5 adversarial | 48 | 21 | 43.8% |
| **all** | **199** | **100** | **50.3%** |

### By plan (post-execution `plan_used`, after any downgrade)

| plan_used | n | hits | hit% | empty |
|---|---|---|---|---|
| Factual | 152 | 86 | 56.6% | 0 |
| Episodic | 2 | 2 | 100% | 0 |
| Affective | 8 | 3 | 37.5% | 0 |
| Abstract | 25 | 9 | 36.0% | 0 |
| **Hybrid** | **10** | **0** | **0.0%** | **10** |

### By outcome

| outcome | n | meaning |
|---|---|---|
| ok | 152 | dispatched cleanly, candidates returned |
| downgraded_from_abstract | 25 | L5 abstract substrate not built — falls to Factual |
| downgraded_from_episodic | 2 | episodic plan unavailable on this substrate version |
| no_cognitive_state | 8 | affective plan needs cog-state, none stored for these |
| empty_result_set | 10 | **all 10 are Hybrid** |

---

## 3. Investigation: Hybrid plan — why 0/10?

### Symptom

10 questions classified as `Hybrid` plan. All 10 return zero candidates (`outcome=empty_result_set`). Question IDs from the run log:

```
[82/197]  cat=4 gold=D2:3   plan=Hybrid empty
[117/197] cat=4 gold=D10:14 plan=Hybrid empty
[144/197] cat=4 gold=D18:5  plan=Hybrid empty
[146/197] cat=4 gold=D18:5  plan=Hybrid empty
[150/197] cat=4 gold=D18:17 plan=Hybrid empty
[151/197] cat=5 gold=D2:3   plan=Hybrid empty
[174/197] cat=5 gold=D10:14 plan=Hybrid empty
[192/197] cat=5 gold=D18:5  plan=Hybrid empty
[194/197] cat=5 gold=D18:5  plan=Hybrid empty
[196/197] cat=5 gold=D18:17 plan=Hybrid empty
```

Note pattern: the same gold dia_ids (D2:3, D10:14, D18:5, D18:17) repeat in cat=4 and cat=5. These are paraphrased / adversarial pairs from LoCoMo's cat=5 construction. Five distinct gold dialogues × 2 phrasings = 10.

### Root cause (preliminary, needs binary verification)

`Hybrid` is the multi-modal plan that was supposed to combine Factual + Abstract + temporal evidence. Looking at the codebase status:

- `Hybrid` is **classified** by `query_classifier` based on linguistic features (multi-clause questions, comparative phrases, "and"/"or" conjunctions).
- `Hybrid` execution path inside `RetrievalEngine::execute_plan`: there is a `HybridPlan` arm, but its sub-plan fan-out depends on having both a working **abstract substrate (L5)** and a working **multi-hop traversal**. Neither is wired:
  - L5 abstract: deferred (see RUN-0009 "downgraded_from_abstract = 25" — Abstract plan itself returns by downgrading to Factual).
  - Multi-hop traversal: ISS-070 — no graph traversal in dispatcher.
- Without either sub-component, `HybridPlan` returns the empty union `{} ∪ {} = {}` and emits `outcome=empty_result_set` instead of downgrading.

**This is a real bug, not a missing feature.** Even with both sub-substrates absent, Hybrid should fall back to Factual (the same way Abstract does), not return empty. Currently it doesn't, which is why we see 10 silent zeros.

### Reproduction

```bash
RUST_LOG=engramai::retrieval=debug cargo run --release \
  --example locomo_conv26_retrieval -- \
  --db .../locomo-conv26-full.db \
  --graph-db .../locomo-conv26-full.graph.db \
  --dataset locomo10.json \
  --max-session 19 --limit 5 --ns locomo-conv26-full
```

The debug log at `engramai::retrieval=debug` will show the per-sub-plan dispatch and confirm whether Hybrid actually fans out or returns empty before any sub-plan executes.

### Proposed fix shape (no code yet)

In `RetrievalEngine::execute_plan` Hybrid arm:

1. Try each sub-plan in priority order (Factual → Episodic → Abstract).
2. If **all** sub-plans return zero candidates, emit `downgraded_from_hybrid` and re-dispatch the original query as Factual (current best-available plan).
3. Never silently return `empty_result_set` from Hybrid unless every sub-plan was actually attempted and all returned zero. Even then, prefer downgrade over empty.

A cheaper interim mitigation: in `query_classifier`, only classify `Hybrid` when at least one of {abstract_available, multi_hop_traversal_available} is true. If neither, classify as Factual directly. This treats classification as substrate-aware.

**To file as ISS-XXX**: "Hybrid plan returns empty_result_set instead of downgrading; add fallback to Factual when sub-plans all return zero."

---

## 4. Multi-hop (cat=1) — what happened to spreading activation?

### Current state on this run

cat=1 multi-hop: 33 questions, 10 hits (30.3%). Plan dispatch:

| plan dispatched | n | hits |
|---|---|---|
| Factual | 26 | 6 |
| Abstract | 5 | 2 |
| Episodic | 2 | 2 |
| **Hybrid** | **0** | — |

**Zero of the 33 multi-hop questions reach Hybrid.** They all classify as Factual or Abstract and use single-shot retrieval. This is exactly the fall-through that **ISS-070** describes: "Multi-hop plan dispatcher has no graph traversal — falls through to single-shot Factual/Abstract."

### What we actually learned from the SA prototype (2026-04-30)

Spreading activation (SA) was prototyped against RUN-0008 substrate. Result: **hit@5 = 1/3 (33.3%)**, and the 1 that hit was effectively a **coincidence**. The full investigation is in `.gid/features/v03-retrieval/INVESTIGATION-2026-04-30-spreading-activation-status.md`. Key findings:

```
q1 "What did Caroline research?"             gold=D2:8   → ✓ hit at rank 3
q2 "What is Caroline's identity?"            gold=D1:5   → ✗ miss
q3 "What is Caroline's relationship status?" gold=D3:13  → ✗ miss
```

The activation traces for **all three questions are identical**, because anchor resolution only matched the entity token "Caroline" and ignored the question-intent words (`research`, `identity`, `relationship`). Diffusion answered "what is most associated with Caroline?" — not "what did Caroline *research*?" q1 hit because D2:8 happened to be the most Caroline-adjacent memory; q2 and q3 missed because the gold facts were not the most Caroline-adjacent.

So **SA's diffusion mechanism is correct**, but **the anchor-resolution spec is incomplete**. The current spec is question-blind: tokenize → name_index lookup → inject as anchors. There is no path for question intent (predicate hints, embedding boost, semantic class) to influence the activation pattern.

This is a **design problem, not an algorithmic one**. We paused SA integration after this run and decided to redesign anchor resolution before continuing — wiring the question-blind prototype into `RetrievalEngine` would just propagate the 1/3 ceiling into RUN-0009.

### Why SA isn't in RUN-0009

Two reasons:

1. The prototype is question-blind (above). Wiring it would not improve cat=1 — it would just standardize the 33% ceiling.
2. The prototype is a standalone binary that talks to `memory.db` / `graph.db` directly via `rusqlite`. It doesn't go through `RetrievalEngine`. ISS-070 ("dispatcher has no MultiHop traversal") remains open precisely because we don't want to integrate the broken-anchor version.

### What's actually needed before SA can move the cat=1 number

In dependency order:

1. **Anchor resolution v2** — design and implement question-aware anchoring. Options: predicate extraction from query, question embedding boost on edge conductance, semantic-class targeting (`research?` → boost `uses`/`depends_on` edges near anchor). Currently a design gap, not an issue yet.
2. **Diffusion convergence** — prototype didn't converge at K_MAX=10 (max_delta=0.0521 still). Either raise K_MAX or steepen decay so rank stabilises.
3. **Then** wire into `RetrievalEngine::MultiHopPlan` — that's ISS-070, blocked on (1) and (2).

### Honest expectation for cat=1

Without (1), cat=1 will not move materially regardless of how we wire SA. The current 30.3% from single-shot Factual is what you get when you ignore graph structure entirely; SA-with-blind-anchors would also land in that range because for half the questions it's effectively single-shot anchored-on-Caroline. The real lift comes from anchor v2 — which has not been designed yet.

---

## 5. Comparison: small-sample vs full-conv

The 3-session 20-QA sample (RUN-0007/0008) reported 50%. Full 150-QA reports 52.7%. The 3pp difference is within sampling noise for n=20, so:

- **Substrate behaviour does not degrade with scale.** 419-turn graph + 113 entities + 116 edges holds up.
- **Small-sample results were predictive.** Future iteration on RUN-0007-style 20-QA samples is a valid fast feedback loop; we don't need to wait 30min for full-conv after every fix.
- **What the small sample missed**: Hybrid 0% and the multi-hop fall-through were invisible at n=20 because too few questions hit those plans. The full run is needed for plan-level coverage, not for headline hit@5.

---

## 6. Open follow-ups

| # | What | Where |
|---|---|---|
| 1 | File ISS for "Hybrid plan emits empty instead of downgrading to Factual" — see §3 | new issue (filed below) |
| 2 | Design **anchor resolution v2** for SA before wiring into pipeline (question-aware anchoring; current spec is question-blind, ceiling 33%) | new design doc, blocks ISS-070 |
| 3 | **Wire engram retriever as a `cogmembench` adapter for conv-26**, then run LLM-as-judge to get J score comparable to Mem0 / MemGPT / Letta. ~$4 / run on 199 QAs. Until this lands we cannot publish cross-system numbers. | cogmembench side |
| 4 | Investigate cat=3 (open-domain) at 33.3% — second-worst category after cat=1; not yet examined | followup |

## 7. Artifacts

- This report: `.gid/eval-runs/RUN-0009-substrate/RUN-0009-full-conv-report.md`
- Raw retrieve log (197 line-items + summary): `.gid/eval-runs/RUN-0009-substrate/RUN-0009-full-conv26.log`
- Ingest stdout: `.gid/eval-runs/RUN-0009-substrate/ingest.stdout.log` (419 turns, 0 failures, 1633.5s, 3.90s/turn)
- Ingest per-call log: `.gid/eval-runs/RUN-0009-substrate/ingest.log`
- Substrate DB: `.gid/eval-runs/RUN-0009-substrate/locomo-conv26-full.{db,graph.db}` — keep until next substrate rebuild
- Retrieve script: `.gid/eval-runs/RUN-0009-substrate/02_retrieve.sh`
- Ingest script: `.gid/eval-runs/RUN-0009-substrate/01_ingest.py`
