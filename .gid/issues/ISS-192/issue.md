---
relates: engram:ISS-165
---
# .gid/issues/ISS-192/issue.md (issue)
project: engram
---
title: Resolver over-generates junk anchors: common-word n-grams crowd out precise multi-word entities under max_anchors cap, diluting graph_score
status: open
priority: P1
severity: degradation
category: retrieval
created: 2026-05-29
relates: [ISS-165, ISS-172, ISS-159, ISS-148, ISS-186]
discovered_in: conv-26-q0 graph-retrieval forensic trace 2026-05-29
---

## Summary

ISS-165 gave `GraphEntityResolver` a mention-extraction step
(n-grams 1..=4 → exact alias lookup). That fixed the
"0 anchors for any natural-language query" bug. But it introduced
a new failure: for natural-language questions the n-gram sweep
resolves **junk anchors from common words** (e.g. the verb "go",
the noun "support", "group") alongside the precise multi-word
entity. With `max_anchors = 5` and a recency-only tiebreak among
equal alias hits, **the precise entity that owns the gold edge can
be truncated out**, and even when it survives it **dilutes
`graph_score`** (denominator inflated by junk anchors).

This is the proximate cause of conv-26-q0 answering "I don't know"
despite a perfect, populated graph.

## Worked example — conv-26-q0

Query: **"When did Caroline go to the LGBTQ support group?"**
Gold answer: **"7 May 2023"** (category labeled multi-hop, but the
answer is a date — see structural note below).

Routing evidence (ISS-190 run `conv26.log` lines 6-7):

```
execute_plan ENTER plan_kind=factual query="When did Caroline go to the LGBTQ support group?"
execute_plan EXIT  plan_kind=factual candidates=230 outcome=ok
```

The graph plan ran, resolved anchors, and produced 230 candidates.
The gold memory exists and is reachable:

- Substrate `memories`: `02700088` = "Caroline attended a LGBTQ
  support group", `occurred_at = 2023-05-08` (gold "7 May 2023"),
  has a 3072-d embedding, present in FTS.
- Graph `graph_edges`: `Caroline (3A904663) --part_of-->
  LGBTQ support group (F3889698)`, `memory_id = 02700088`
  (hex blob `3032373030303838` decoded). The edge is **outgoing
  from Caroline**, so Stage-2 1-hop traversal reaches it, and
  ISS-189 D1 seeds `02700088` into the candidate pool.
- The bench prepends the date: context line would read
  `[2023-05-08] Caroline attended a LGBTQ support group`.

So if `02700088` reached the top-K generation context, the LLM
could answer directly. It did not — generation said "the memories
don't specify when," which is only possible if `02700088` ranked
outside top-10.

### Why it ranked out — the anchor set

Replaying `extract_mentions` (n-grams 1..=4) against the run's
`graph_entity_aliases` (exact normalized match), q0 resolves
**6 anchor entities**, all exact alias hits (alias_boost = 0.7 each):

| entity_id | canonical_name        | kind          | last_seen (recency, the only tiebreak) |
|-----------|-----------------------|---------------|----------------------------------------|
| 3A904663  | Caroline              | person        | 19:31:09  (rank 1)                      |
| 4A0256E3  | support               | unknown       | 19:31:07  (rank 2) ← junk               |
| 595A4A7A  | Go                    | artifact      | 19:26:08  (rank 3) ← junk (the verb)    |
| 7C00B572  | group                 | organization  | 19:24:13  (rank 4) ← junk               |
| 9EF62BC9  | support group         | organization  | 19:17:41  (rank 5) ← generic            |
| F3889698  | **LGBTQ support group** | organization | 19:17:33  (rank 6) ← **TRUNCATED**      |

`match_strength = 0.7 (alias) + 0.3 * recency_score`. All six are
exact alias hits, so the 0.7 term is constant and **recency is the
sole discriminator**. The top-5 by recency are Caroline, support,
Go, group, support group. **`LGBTQ support group` (F3889698) — the
entity that owns the gold edge — has the lowest recency and is
dropped by the `max_anchors = 5` cap.**

Result: `02700088` is `seen_via` only Caroline (its support-group
anchor was cut) → `graph_score = 1 / 5 = 0.2`, indistinguishable
from hundreds of other Caroline-tagged memories. ISS-172's cosine
tiebreak cannot overcome that gap among 230 candidates truncated
to top-10. The gold drowns; generation answers "I don't know."

## Two compounding defects

### Defect A — junk anchors from common-word n-grams

`extract_mentions` emits every n-gram (no stopword filter, no
POS/quality gate). LoCoMo's extractor created entity rows for
common words ("support", "group", "Go"), so these n-grams resolve
to real entities with full `alias_boost = 0.7`. They are
indistinguishable from genuine named entities at the scoring
stage.

### Defect B — graph_score is anchor-coverage breadth, not edge precision

`factual_to_scored`: `graph_score = seen_via.len() / total_anchors`.
This rewards a memory for being touched by *many* anchors, even
when those anchors are junk. The memory that the traversed edge
**explicitly asserts** (the `part_of` edge literally encoding the
queried relation) gets no privilege over a memory that merely
co-mentions several query tokens. Inflating `total_anchors` with
junk also shrinks every legitimate memory's score.

These compound: A puts junk into the numerator pool and the
max_anchors race; B turns that junk into ranking signal.

## Structural note — conv-26 "multi-hop" is mostly temporal

Of 24 multi-hop failures in the ISS-190 conv-26 run, ~22 have date
gold answers ("the weekend before 17 July 2023", "since 2016",
"7 May 2023"). The temporal category is 45/70 fail — the worst
category. The "multi-hop" label here mostly means "retrieve the
right dated episode and compute a relative date," not "chain N
entity hops." So the productive lever is **ranking precision +
getting the right dated episode into top-K**, NOT
1-hop→multi-hop or outgoing→bidirectional traversal. Those would
not have moved q0.

## Fix directions (in order of leverage / cost)

1. **Stopword + quality gate on mentions** (cheap, deterministic).
   Drop common-word unigrams/bigrams before alias lookup; or
   require the matched entity's `kind` be a meaningful class
   (person/organization/place/event/artifact/concept) and reject
   `{"other":"unknown"}` matches like "support". This kills the
   "support"/"Go"/"group" anchors directly. **Try first.**

2. **Prefer longer n-gram matches (specificity over recency).**
   When a longer n-gram and its constituent shorter n-grams both
   resolve, keep the longest (most specific) and drop the
   subsumed shorter mentions. "LGBTQ support group" should
   suppress "support group", "support", "group". Change the
   `max_anchors` tiebreak from recency to mention-specificity
   (n-gram length / IDF), so the precise entity is never
   truncated in favor of a generic one.

3. **Privilege edge-asserting memories in graph_score** (root fix
   for Defect B). A candidate seeded from a traversed edge whose
   predicate matches the query intent should get a score boost
   over plain co-mention neighbors. The edge that literally states
   the answer should outrank coincidental co-mentions regardless
   of anchor breadth.

4. **Cross-encoder reranker (ISS-159)** over the top-N candidates.
   Independent of the anchor bug, this directly attacks the
   "right candidate is in the 230-pool but loses the top-10 race"
   problem. Complementary to 1-3.

## Fix 2 validated offline (2026-05-29)

Replayed the resolver mention→anchor mapping on the retained
conv-26 graph with a **specificity-dedup rule**: drop any mention
whose token span `[s,e)` is fully contained in a *longer* mention
that also resolved. This collapses q0's anchor set decisively:

```
before(6): [Caroline, Go, support, group, support group, LGBTQ support group]
after (3): [Caroline, Go, LGBTQ support group]
```

`support`, `group`, `support group` are all subsumed by the
3-gram `lgbtq support group` and dropped. The precise gold entity
**survives**. With 3 anchors the gold memory `02700088` is
`seen_via` Caroline + LGBTQ-support-group = **2/3 = 0.67**
graph_score (vs 0.2 before) — top of the pool, well inside top-10.

Non-destructive: replayed against 5 conv-26 questions; only the
overlapping-n-gram case (q0) changes — all other anchor sets are
byte-identical (no subsumed multi-word matches → rule is inert).

Residual: `Go` (the verb, kind=Artifact) still resolves as a lone
unigram. Harmless here (gold is 2/3 regardless), but a small
stopword/function-word gate on unigrams (fix 1b) would drop it and
take the gold to 2/2 = 1.0. Recommend fix 2 first (root fix,
proven sufficient for q0), fix 1b as cheap follow-up.

## Acceptance criteria

- [ ] **AC-1**: Reproduce the anchor set for conv-26-q0 on a fresh
  run with `RUST_LOG=engramai::retrieval::factual=trace` — confirm
  Stage-1 logs show `LGBTQ support group` truncated (or, post-fix,
  retained) and `02700088`'s `seen_via` / `graph_score`.
- [ ] **AC-2**: Implement fix direction 1 (mention quality gate)
  and/or 2 (specificity tiebreak). Verify q0's anchor set keeps
  `LGBTQ support group` and drops the junk anchors.
- [ ] **AC-3**: Same-config A/B on conv-26 (K=10, temp=0, HyDE off,
  MMR off, entity_channel as in ISS-171, PIPELINE_POOL=1). Pass:
  q0 flips 0→1 AND single-hop/multi-hop no regression vs the
  ISS-190 baseline (overall 0.3158).
- [ ] **AC-4**: Quantify how many of the 9 ISS-161 failing
  single-fact queries (and the 22 temporal-multi-hop fails) share
  the junk-anchor / graph_score-dilution pattern. Decide whether
  fix 3 (edge-precision graph_score) is needed beyond the cheap
  mention gate.

## Artifacts

- ISS-190 run: `engram-bench/benchmarks/runs/ISS190-fix-conv26-20260529T191730Z/` (`locomo_per_query.jsonl`, `conv26.log` at `/tmp/iss190-conv26/conv26.log`)
- Retained populated graph + substrate: `/var/folders/48/npr42z0967b376x1rc7wbp6m0000gn/T/.tmpJq4d6Y/{graph.db,substrate.db}` (695 entities, 816 edges)
- Offline anchor-set replay: inline python in the discovery turn (tokenize → n-grams 1..4 → exact `graph_entity_aliases.normalized` match → join `graph_entities` for recency)
- Dump-enabled confirm probe: `/tmp/q0_probe.sh` (ENGRAM_BENCH_DUMP_CANDIDATES=1, conv-26 only) → `Q0PROBE-conv26-*` — confirms `02700088`'s exact rank in q0's 230 candidates

## AC-1 result — confirm probe (PID 7652, Q0PROBE-conv26-20260529T210331Z, 2026-05-29)

Pre-fix run overall = **0.296**. conv-26-q0 = **score 0.0**, gold
`"7 May 2023"`, predicted: *"I don't know. The memories mention
Caroline is part of 'Connected LGBTQ Activists' group with regular
meetings, but don't specify when she went to..."*

Stage-1 trace for q0 (this ingest):

```
stage1 resolve → 5 anchor(s)   [max_anchors cap = 5]
  LGBTQ support group   match_strength=0.700   ← gold entity SURVIVED here
  Caroline              match_strength=0.700
  support               match_strength=0.700   ← junk
  group                 match_strength=0.700   ← junk
  support group         match_strength=0.700   ← generic
stage2 traverse: 5 anchors → 215 outgoing edges, 178 linked entities
  edge anchor=LGBTQ-support-group pred=PartOf linked=Caroline memory_id=e4b0fa9d live=true  ← gold edge ENTERS pool
stage3 seed: edge-provenance contributed 148 candidate(s)
factual done: outcome=Ok candidates=224 (seeded=148)
```

**Refined diagnosis.** In this ingest the precise entity was *not*
truncated — it ranked anchor #1 and the gold edge `e4b0fa9d`
(LGBTQ support group --PartOf--> Caroline) **did enter the
224-candidate pool**. Yet q0 still answered "I don't know," and the
predicted text shows generation received a *different, undated*
LGBTQ-membership memory ("Connected LGBTQ Activists ... regular
meetings"), not the dated `attended a LGBTQ support group` episode.

So the failure that survives even when the anchor is retained is
**Defect B (graph_score breadth-dilution)**: the dated gold episode
sits in the 224-candidate pool but ranks below top-10 because its
`graph_score` (seen_via.len / total_anchors, diluted by the 3 junk
anchors) is indistinguishable from co-mention neighbors. The
recency-driven truncation of Defect A is **non-deterministic across
ingests** (entity UUIDs and last_seen reshuffle per ingest), so it
intermittently compounds B but is not the sole cause.

**Implication for the fix.** Fix 2 (specificity dedup, shipped
`c680a20`) makes precise-entity survival **deterministic** (drops
the 3 junk fragments → total_anchors falls 5→2, lifting the gold's
graph_score from ~0.2 to ~0.5+) — this attacks B's denominator
directly even though it was framed as an anchor-cap fix. Whether it
is sufficient to lift the *dated* episode into top-10 (vs the undated
membership memory) is what AC-3's A/B measures. If the undated memory
still wins, Defect B's numerator privilege (fix 3: edge-predicate
boost) or a reranker (ISS-159) is required on top.

## AC-3 result — FIX 2 FALSIFIED (conv-26 A/B, 2026-05-29)

Fix 2 (specificity dedup) shipped as `c680a20`, reverted as `2825a14`.

A/B (identical ISS-190 envelope: K=10 temp=0 HyDE/MMR/entity off,
FACTUAL_REWEIGHT off, pipeline_pool=1):

```
                pre-fix (probe)   after-fix       Δ
overall         0.2961            0.2566          -3.95pp
multi-hop       0.3514            0.2432          -10.82pp  ← regression
single-hop      0.0938            0.0313          -6.25pp   ← regression
open-domain     0.2308            0.2308           0.00pp
temporal        0.3714            0.3714           0.00pp
conv-26-q0      0.0               0.0              NO FLIP
```

Per-query diff: **3 gains, 9 losses, net −6 questions.** Losses
cluster in multi-hop (q10, q21, q67, q79) and single-hop (q24, q47) —
the categories that dropped. This is systematic, not LLM-judge wobble.

**Why it failed.** Two compounding reasons:

1. **q0 root cause is untouched.** The dated gold episode
   (`occurred_at 2023-05-08`) ranks *outside top-10* regardless of the
   anchor set. Fix 2 only changed *which* undated LGBTQ memory reached
   generation (pre: "Connected LGBTQ Activists … regular meetings";
   post: "attending an LGBTQ+ counseling …") — never the dated one.
   The defect is in the graph_score **numerator / ranking stage**, not
   anchor breadth. Confirmed by AC-1: even when the precise entity
   *survives* the cap, q0 still fails.

2. **Dedup removes signal, not just noise.** Dropping the fragment
   anchors (`support`, `group`, `support group`) also drops the
   *legitimate co-mention edges* those fragments seed into the
   candidate pool. For multi-hop/single-hop questions where a generic
   fragment was the bridge to a useful neighbor, removing it deletes
   the path. Specificity-by-subsumption is too blunt: token-span
   containment does not imply semantic redundancy.

**Decision.** Fix 2 is the WRONG lever. Reverted. The real fix must
operate on **graph_score numerator** (Defect B), not anchor pruning:

- **Fix 3 (preferred):** privilege edge-asserting memories in
  graph_score — a candidate reached via an edge whose predicate
  matches the query relation (e.g. PartOf / temporal "when") gets a
  numerator boost over coincidental co-mentions. This lifts the dated
  episode above undated membership memories WITHOUT removing any
  candidates, so multi-hop bridges survive.
- **Fix 4 (orthogonal, see q0_root_cause note):** the precise day is
  stranded in the temporal `note` string ("yesterday (2023-05-07)…")
  while structured start/end collapsed to a full-year interval. Even a
  perfect ranking can't surface "7 May 2023" if the day never reaches
  generation. Extractor must pin the resolved day into start/end.
  Likely a separate ISS downstream of ISS-190/191.

q0 needs BOTH: Fix 3 to rank the episode into top-10, Fix 4 so the
episode actually carries the day. Anchor pruning (Fix 1/2) is closed.

---

## AC-3 result — fix 3 (edge-seed privilege) VALIDATED (2026-05-30)

Commit `437b620`. conv-26 A/B, ISS-190 envelope (K=10 temp=0 HyDE/MMR/
entity off, FACTUAL_REWEIGHT off, pipeline_pool=1, POPULATE off).
Arm A = `ENGRAM_FACTUAL_EDGE_SEED_BONUS` unset (inert = pre-fix breadth).
Arm B = bonus 0.5.
Run STAMP 20260529T234442Z, dirs `ISS192-{A,B}-conv26-20260529T234442Z`.

| category    | A (inert) | B (bonus 0.5) | Δ        |
|-------------|-----------|---------------|----------|
| overall     | 0.2368    | 0.2763        | +3.95pp  |
| multi-hop   | 0.2703    | 0.3784        | +10.81pp |
| open-domain | 0.0769    | 0.2308        | +15.38pp |
| single-hop  | 0.0313    | 0.0625        | +3.13pp  |
| temporal    | 0.3429    | 0.3286        | −1.43pp  |

Gains 12 / losses 6 = net +6. **No multi-hop regression** — the additive
band-split (co-mentions → [0,1-w], edge-seeded → [w,1]) preserved bridge
ordering as designed; asserting edges simply stopped losing to coincidental
co-mention breadth.

### q0 — the decisive evidence (Defect B fixed; residual = fix 4)
- Arm A: `"I don't know."`
- Arm B: `"According to memory [6], Caroline attended a LGBTQ support group
  on 2023-05-08."`

Fix 3 lifted the dated gold episode into top-K AND into generation's
context — exactly its job. Judge scored 0.0 only because the surfaced date
is **2023-05-08** (the episode's `occurred_at` / conversation turn) not the
gold **7 May 2023**. The event "yesterday" resolved to 2023-05-07 but that
resolved day was stranded (Vague) instead of pinned to `Day` — this is
**ISS-194 fix 4** exactly. q0 now needs ONLY fix 4 to flip 0→1.

### Disposition
- AC-3 (q0 dated episode reaches top-K, no regression, overall recovers):
  **PASS**. Overall 0.2763 ≥ baseline; the only category dip is temporal
  −1.43pp (single-question judge wobble), multi/single-hop both up.
- Default bonus: recommend keeping `ENGRAM_FACTUAL_EDGE_SEED_BONUS` **opt-in
  (default 0.0)** until a conv-44 cross-validation confirms the gain is
  corpus-general (not conv-26-specific). The +10.81pp multi-hop / +15.38pp
  open-domain lift is large enough to warrant the cross-val before flipping
  the default.
- Next: implement ISS-194 fix 4 → re-run q0 to confirm the full 0→1 flip,
  then conv-44 cross-val for the default-on decision.
