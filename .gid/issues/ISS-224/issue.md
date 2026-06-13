---
title: 'conv-26 recall ceiling: half of gold evidence is absent from top-200 before reranking — the real LoCoMo bottleneck is recall+grounding, not ranking/K/generation'
status: open
priority: P1
severity: major
labels: [locomo, recall, extraction, grounding, diagnosis]
feature: v04-unified-substrate
created: 2026-06-13
relates_to: [ISS-201, ISS-223, ISS-186, ISS-190, ISS-191, ISS-141, ISS-203, ISS-202]
---

# Summary

ISS-201 closed every retrieval-side ranking lever (Hybrid-pool widening, CE
seed-pool deepening, global K=30, targeted list-K) and ISS-223 closed MMR-λ.
Three independent diagnostic lines (list-scatter, list-absence, temporal+IDK)
all converged on the **same** root cause: the gold evidence is frequently **not
in the candidate pool at all** before any reranking happens. Ranking, K-sizing,
fusion, MMR, and generation-prompt work cannot move a candidate that was never
retrieved.

This issue tracks the actual frontier — **recall + grounding** — decomposed
into three concrete, separately-attackable sub-directions (A date-grounding,
B query↔document asymmetry, C entity-memory disambiguation). It is the
successor to ISS-201's now-exhausted retrieval-ranking scope.

# Evidence: the recall ceiling

Pure bi-encoder top-200 recall probe (`iss186_candidate_pool_probe`, bypasses
fusion/CE/MMR), conv-26 all 152 questions, run `conv26-all-20260613T034146Z`:

- single-hop  recall@200 = **0.312** (68% of gold MISSED beyond top-200)
- open-domain recall@200 = **0.000** (gold NEVER in top-200 for any of 13)
- temporal    recall@200 = 0.471
- multi-hop   recall@200 = 0.919  (the one healthy category)
- **ALL       recall@200 = 0.507**

Half of all gold evidence is absent from the top-200 **before any reranking**.
Aggregate end-to-end J-score on the clean LEVER2 envelope is 0.5329
(`ISS222-LEVER2-conv26-20260612T002433Z`) — i.e. end-to-end is already close to
the bi-encoder recall@200 ceiling, confirming recall (not ranking) is the wall.

Note the conv-26 achievable ceiling is **< 100%**: some gold answers are
ungroundable (e.g. q34 "school speech" appears 0× in all 419 episodes). Any
recall target metric must exclude the ungroundable subset.

# Sub-direction A — date-grounding (temporal recall)

**Symptom.** temporal failures = 25/70; **18 of those 25 are IDK refusals**, and
the recall cross-check shows 13 of the 18 have gold absent from top-200. The
event memory is often retrieved but its text/embedding carries **no date**, so a
temporal query ("what did X do on/around <time>") cannot match it.

**Root.** The extractor resolves relative expressions ("last Saturday",
"yesterday") into a `note` string but does **not** pin the resolved day into the
memory text or the structured `start/end` interval (start/end collapse to a
full-year span). Generation reads text-only; temporal scoring reads start/end —
the precise day is captured at extraction then discarded into a comment nobody
consumes (see q0 root-cause analysis).

**Prior art / gap.** ISS-190/191 handled `duration → year` (e.g. "owned for 3
years" + occurred_at → ~2020) and added the typed `TemporalMark::Approx`
variant + interval-overlap `temporal_score`. The remaining gap is
**event-relative → day** ("yesterday (2023-05-07) relative to 2023-05-08"):
narrow the interval to the resolved day AND surface the date in the memory text
so the bi-encoder can match it.

**Next step.** Probe: of the 13 absent temporal golds, how many have a source
episode with a resolvable relative date that the extractor dropped? If the
majority — extractor day-pinning is the lever.

# Sub-direction B — query↔document asymmetry (event vs preference)

**Symptom.** Questions ask about a **preference/category** ("What do Melanie's
kids like?") but the source episode states a one-time **event** ("They were
stoked for the dinosaur exhibit"). The token is present in the corpus but ranks
> 200 because event-phrasing embeds far from preference-phrasing. Confirmed on
q19 ("dinosaurs" present in ep97, ranks > 200).

**Candidate levers.**
- Query-side: query expansion / HyDE that bridges preference→event ("things kids
  like" → hypothetical "kids enjoyed/were excited about …"). ISS-141 partly
  explored HyDE as a query-layer concern; revisit specifically for this bridge.
- Extraction-side: emit a derived **preference** memory when an episode
  expresses sustained enjoyment of an activity ("love learning about animals" →
  preference: nature/animals), not just the event record.

**Next step.** Decide query-layer (HyDE bridge) vs extraction-layer (derived
preference memories); the latter is more general but heavier. A/B both on the
B-affected qid subset.

# Sub-direction C — entity-memory disambiguation

**Symptom.** Retrieval lands on the **correct entity** but selects the **wrong
memory** about that entity. q93 (gift from grandma, gold "necklace") retrieves
the "friend's hand-painted bowl" memory and the generator honestly answers "no
mention of a gift from grandma". q94 (gold "art and self-expression") retrieves
Melanie's bowls/paintings but not the specific memory that says "the bowl
reminds me of art and self-expression" (ep62). The generator's IDK here is
**honest and correct** — it lacks the gold memory.

**Root hypothesis.** Many memories share the same anchor entity; the
discriminating detail (grandma vs friend; which object the reminder attaches to)
is under-weighted by the bi-encoder relative to the dominant entity term. This
is a precision-within-entity problem, distinct from A (missing date) and B
(missing the memory entirely from a phrasing gap).

**Next step.** Quantify how many of the C-class failures have the gold memory
present at rank 11–200 (recoverable by a better within-entity discriminator)
vs > 200 (a deeper recall problem). The probe data already exists; just
re-bucket the IDK-but-entity-matched set.

# What is NOT the bottleneck (closed by ISS-201/ISS-223)

_TBD_

# Acceptance criteria

_TBD_
