---
title: 'conv-26 LoCoMo failure decomposition: 3 distinct layers (retrieval short-circuit / generation over-caution / judge wobble), NOT a single bottleneck'
status: open
priority: P1
severity: major
labels:
- v04-unified-substrate
- locomo
- diagnosis
- retrieval
- generation
feature: v04-unified-substrate
created: 2026-05-31
relates_to:
- ISS-198
- ISS-186
- ISS-149
- ISS-161
- ISS-179
- ISS-188
- .gid/issues/ISS-225/issue.md
---

# Summary

Per-memory dissection of the ISS-198 post-fix conv-26 smoke run
(`ISS198-smoke-conv26-20260531T170624Z`, overall **0.283**, default unified
reads on the `nodes` table) decomposes the 109 failing queries into **three
distinct failure layers**, not one. An earlier diagnosis (compaction summary,
same session) concluded the failures were *purely* generation+judge. That was
**half right** — it sampled q47/q48 which happen to fall in the generation
bucket, and missed an equally-large retrieval bucket. This issue records the
corrected, fully-quantified decomposition.

**ISS-198 batch-2 is confirmed clean** — none of these failures are caused by
the FK re-points. The substrate ingests the gold evidence correctly; the
losses are downstream of storage.

# Method

- Source run: `engram-bench/benchmarks/runs/ISS198-smoke-conv26-20260531T170624Z/2026-05-31T17-32-53Z_locomo/locomo_per_query.jsonl` (152 q)
- Live substrate: `/var/folders/48/.../.tmp0VvSQB/substrate.db` (456 memory nodes, unified `nodes` table)
- Envelope: K=10, temp=0, HyDE off, MMR off, entity_channel off, FACTUAL_REWEIGHT off, pipeline_pool=1, POPULATE off (matches ISS-190/161 baseline)
- Classification lever: **`latency_retrieve_ms`**. PASS queries have median retrieve **535ms**; FAIL median **393ms**; and a sharp sub-population of fails retrieve in **<150ms** (median all = 407ms, min = 40ms). A <150ms retrieve means the pipeline short-circuited *before* doing real FTS/vector work.

# The decomposition (109 fails)

| Bucket | Count | retrieve | predicted | Layer |
|---|---|---|---|---|
| **Retrieval short-circuit** | **30** | <150ms | "I don't know" | retrieval / plan-classifier |
| Generation over-caution | 41 | ≥150ms | "I don't know" | generation |
| Wrong-memory / synthesis | 36 | ≥150ms | wrong answer | generation |
| Low-retrieve + wrong | 2 | <150ms | wrong answer | retrieval |

Correlation that pins the retrieval bucket: **32 fails retrieve in <150ms, but only 2 *passes* do.** Low retrieve latency is an almost-perfect failure predictor — when retrieval finishes that fast it returned little/nothing, and generation correctly says "I don't know."

# Smoking-gun evidence (retrieval bucket — gold ingested, never surfaced)

All three below returned "I don't know" with a <70ms retrieve, yet the gold
content is verifiably present in the substrate AND findable via `nodes_fts`:

- **q52** "What are Melanie's pets' names?" gold=`Oliver, Luna, Bailey`, evidence `[D13:4, D7:18]`, retrieve **55ms**.
  Substrate has 4 memories: `Melanie has two cats: Oliver and Bailey`, `Luna and Oliver! They are so sweet and playful`, `Oliver's favorite food...`, `Oliver hid his bone...`. `nodes_fts MATCH 'cats'` returns the Oliver/Bailey memory; `MATCH 'pets'` returns 6 hits. Embeddings present in `node_embeddings` for all of them. → ingestion ✓, FTS-findable ✓, **retrieval returned nothing**.
- **q106** "What are the new shoes Melanie got used for?" gold=`Running`, evidence `[D7:19]`, retrieve **60ms**. Substrate has 6 `running` memories.
- **q118** "What did Melanie and her family see during their camping trip last year?" gold=`Perseid meteor shower`, evidence `[D10:14]`, retrieve **62ms**. Substrate has 2 `Perseid/meteor` memories.

Contrast: **q4** (PASS, gold `Transgender woman`) retrieved in **1094ms** — the full pipeline ran.

## Likely root cause of the retrieval bucket

The <150ms short-circuit is consistent with the long-standing **plan-classifier
death** theme (ISS-149): these single-hop/temporal queries get routed to a plan
that produces few/zero candidates instead of running FTS+vector fusion. The data
is reachable (proven above); the *plan* never reaches it. This is the same class
ISS-186 (candidate-pool recall) targets. **Next probe: re-run the 32 low-retrieve
queries with `ENGRAM_BENCH_DUMP_CANDIDATES=1` and inspect `execute_plan ENTER
plan_kind` — confirm which plan they route to and whether the candidate list is
empty.**

## ROOT CAUSE CONFIRMED (2026-05-31, replay probe AC-1)

Built `engram-bench/examples/iss201_replay_probe.rs` — opens the ALREADY-INGESTED
substrate.db (no 12-min re-ingest), chains `.with_graph_store(&db)` (single-file
co-located), replays the failing queries through `graph_query_locked`, prints
`plan_used` + per-candidate fused score + whether the gold memory landed in
top-10. Full output: `artifacts/replay-probe-20260531.txt`. Findings:

| query | plan | retrieve_ms | top score | gold in top-10 |
|---|---|---|---|---|
| q4 (PASS control) | **Factual** | 1067 | 0.84 | n/a (different gold) |
| q52 "Melanie's pets' names?" | **Hybrid** | 40 | 0.0167 (RRF) | **NO** |
| q106 "new shoes used for?" | **Hybrid** | 34 | 0.0167 (RRF) | **NO** |
| q118 "camping trip... see?" | **Hybrid** | 34 | 0.032 (RRF) | **NO** |
| q52-paraphrase "names of Melanie's **cats**?" | **Abstract** | 134 | 0.9463 (tied) | **YES @ rank 3** |

Three concrete, code-level mechanisms — NOT generation, NOT judge:

1. **Plan misrouting on phrasing.** "Melanie's pets' names" → `Hybrid`; the
   semantically-identical "names of Melanie's cats" → `Abstract`. The intent
   classifier is brittle to surface form. The Factual path (which *works*, q4 =
   1067ms full vector recall) is never selected for these factual lookups.
2. **Hybrid drops the vector channel.** q52/q106/q118 retrieve in **34-40ms** —
   far too fast to have embedded the query (q4's Factual took 1067ms; the
   Abstract paraphrase took 134ms because it *did* vector-search). The Hybrid
   sub-plan selection (`hybrid.rs::select_subplans(signals, tau_high)`) picks
   only cheap FTS/graph sub-plans for these queries, so the candidate pool is
   noise that never contains the gold. RRF then just reorders noise (scores
   ~0.016 = `1/(rrf_k+rank)`, normal RRF magnitude — the low score is not a bug,
   the *empty-of-gold pool* is).
3. **Abstract plan score-ties.** When the paraphrase *did* route to `Abstract`
   and *did* surface the gold ("Melanie has two cats: Oliver and Bailey" at
   rank 3), **every candidate scored exactly 0.9463** — L5 topic similarity is
   degenerate (single super-cluster, cf. RUN-0026 / ISS-106 note), so ranking
   within the pool is arbitrary and the gold rarely reaches top-1.

The fix surface is therefore the **classifier routing + Hybrid sub-plan
selection**, not the storage or generation layers. ISS-186 (pool recall) and
ISS-149 (classifier) are the right tracks; this issue supplies the concrete
reproduction + the replay-probe tool to verify any fix in seconds.

# Generation-layer evidence (the 41 over-caution + 36 wrong buckets)

These DID retrieve (≥150ms) but still failed:

- **q0** (multi-hop) retrieved the "Caroline feels accepted by the support group" + "gained courage to embrace herself" memories but **refused to infer** the date (gold `7 May 2023`) — over-caution refusing cross-memory inference. (NB: q0's gold date is itself stranded in a free-text `note` field per the `q0_root_cause_2026-05-29` finding — so q0 is *partly* a temporal-extraction issue too.)
- **q47** "Where did Caroline get support?" gold=`mentors, family, friends` — substrate has near-verbatim `Caroline received support from friends and mentors`, but generation picked the support-group memory instead → **wrong-memory selection** (could be retrieval ranking OR generation synthesis — open question).
- **q48** gold=`bowls, cup` — all 3 gold memories (pots/cup/bowl) stored; this was a judge wobble vs ISS-161 (substantively identical answer scored differently at temp=0).

# Verdict

- **ISS-198 batch-2: CLEAN.** No failure traces to the FK re-points. Substrate ingestion is correct across every gold checked.
- The conv-26 deficit is **two roughly-equal levers**, not one:
  1. **Retrieval short-circuit (~30 q, P1):** highest-confidence, cleanest fix surface — gold is provably reachable, the plan just doesn't fetch it. Overlaps ISS-149 / ISS-186.
  2. **Generation (~77 q, P1):** over-caution (41) + wrong-memory/synthesis (36). Softer surface; partly entangled with judge wobble at temp=0.
- The earlier "purely generation+judge" verdict is **superseded** — it under-counted the retrieval bucket because it sampled only generation-bucket queries.

# Acceptance criteria

- [x] AC-1: Replay the low-retrieve queries against the live substrate; confirm plan_kind + candidate composition. **DONE** — `iss201_replay_probe.rs`, artifacts/replay-probe-20260531.txt. Proven: Hybrid plan, 34-40ms (no vector search), gold absent from pool; misrouting + dropped-vector-channel are the mechanisms.
- [ ] AC-2: Quantify how many of the 30 retrieval-bucket golds are FTS-reachable (spot-checked 3/3 so far) vs vector-only-reachable.
- [ ] AC-3: Decide lever priority: fix plan-classifier routing (ISS-149 track) vs candidate-pool recall (ISS-186 track) — whichever recovers more of the 30.
- [x] AC-4: Separate the 36 wrong-memory cases into retrieval-ranking (gold not in top-K) vs generation-synthesis (gold in top-K, wrong pick) — extend the replay probe to those queries. **DONE** — see "AC-4: wrong-memory bucket decomposition" below.

## AC-4: wrong-memory bucket decomposition (2026-05-31, replay probe JSON mode)

Extended `iss201_replay_probe.rs` with a `QUERIES_JSON` mode that reads
`[{id,question,gold,evidence,category}]` and emits one JSON line per query with
plan + top-10 candidate contents + scores. Drove it with the 36 wrong-memory
queries (`/tmp/iss201_ac4_queries.json` → `/tmp/iss201_ac4_dump.jsonl`).

**All 36 routed to `Factual`** — the *good* plan — and 35/36 ran full vector
search (>150ms). So this bucket has NO plan-misrouting / dropped-vector problem
(unlike the 30-query retrieval-short-circuit bucket). The gold-bearing memory is
almost always retrieved. The bucket splits three ways:

1. **Temporal date-stranding (~16 q):** "When did X?" questions whose gold is a
   date. The correct *event* memory is retrieved **at rank 0** (q5 "Melanie ran a
   charity race", q8 "Caroline gave a talk at a school event", q44 "Melanie
   celebrated her daughter's birthday") — but the candidate text **carries no
   date** (0-1/10 candidates have any date). The episode's `occurred_at` didn't
   survive into the stored memory text, so generation can't answer "when". This
   is the **same root as conv-26-q0** (date stranded in a free-text `note` while
   structured start/end collapsed to full-year) — tracked by the
   `q0_root_cause_2026-05-29` finding + ISS-190 / ISS-191. **NOT a retrieval
   failure, NOT a generation-synthesis failure** — it's extraction.
2. **Ranking / synthesis / list-completeness (~19 q):** content questions where
   a strongly-relevant memory is in top-K but (a) the wrong one ranks first
   (q47 "where did Caroline get support" → hike memory at rank 0, while the
   "friends, family, and mentors" memory exists in the corpus), or (b) the gold
   is a *list* ("pottery, camping, painting, swimming") and only some items are
   retrieved/synthesized. This IS the generation-synthesis + ranking layer.
3. **Semantic-precision retrieval miss (~1 q):** q56 "What symbols are important
   to Caroline?" gold "Rainbow flag, transgender symbol" — all 10 candidates
   score a healthy 0.80-0.85 (embedder thinks everything Caroline-related is
   equally relevant) and the exact gold memory isn't clearly in top-10. A
   precision/discrimination problem, not a recall one.

**Refined verdict:** the "36 wrong-memory" cases are NOT one layer. ~16 are
extraction (temporal), ~19 are generation/ranking, ~1 is embedder precision.
The single largest *fixable* sub-lever across the whole conv-26 deficit is
**temporal date-stranding** — it shows up in the retrieval bucket (q106/q118
phrased temporally) AND dominates the wrong-memory bucket (16/36). Fixing
extraction to pin resolved dates into memory text/structured fields would lift
both buckets.

# Artifacts (AC-4)

- `/tmp/iss201_ac4_queries.json` — the 36 wrong-memory queries + gold + evidence
- `/tmp/iss201_ac4_dump.jsonl` — per-query plan + top-10 candidate dump
- `engram-bench/examples/iss201_replay_probe.rs` (commit on engram-bench) — JSON mode

# Artifacts

- `benchmarks/runs/ISS198-smoke-conv26-20260531T170624Z/.../locomo_per_query.jsonl`
- `benchmarks/runs/ISS161-A-conv26-20260526T121230Z/locomo_per_query.jsonl` (baseline 0.362)
- Live substrate `/var/folders/48/.../.tmp0VvSQB/substrate.db` (ephemeral — re-create via `/tmp/iss198_smoke.sh`)

## ISS-201 retrieval-short-circuit: ROOT CAUSE = INGEST WINDOWING + PROVEN FIX (2026-06-03)

The 30-query retrieval-short-circuit bucket was further decomposed (embedding-cosine
gold detection, `iss201_shortcircuit_classify2.rs`, GOLD_COS=0.65) into 3 sub-buckets:
**8 OK-gold-retrieved / 6 REACHABLE-misrouted / 18 SEMANTIC-GAP**. The dominant
SEMANTIC-GAP sub-bucket was traced to a **bench-driver ingest defect**, not retrieval.

### Shape-scan (the ceiling)

Mapped all SEMANTIC-GAP gold-turns back to the fixture (questions carry
`evidence: ["Dx:y"]`; 19 sessions D1-D19 = distinct `occurred_at` groups).
**14/14 mappable gold-turns are COREF-DEPENDENT** — the gold fact lives in a bare
reply turn whose subject/object is established in a *preceding* turn (7/14 even
start "Yeah/Thanks/Aww" + pronoun). E.g. q3 "What did Caroline research?" gold turn
is `Caroline: Researching adoption agencies` (no actor in turn); q52 gold is
`Luna and Oliver! They are so sweet` ("their" → question turn). The bench driver
(`engram-bench/src/drivers/locomo.rs:1213`) extracted each turn **in isolation**, so
the bare reply lost its referent and the self-contained gold fact was never stored.

### Fix: sliding-window ingest (env-gated A/B)

`ENGRAM_BENCH_INGEST_WINDOW=N` prepends the N preceding turns as a *reference-only*
context block ("do NOT extract from these — resolve coref against them; extract ONLY
this turn"), `occurred_at` anchored to the answer turn. N=0/unset = byte-identical.

**LLM-level verify (before re-ingest): 4/4 RETRIEVABLE** via exact production framing
(`iss201_window_verify.rs`): q3→"Caroline is researching adoption agencies",
q52→"Melanie has...a cat named Bailey and a cat named Oliver", q122/q140 coref
resolved. Context turns were NOT double-extracted.

### A/B result (conv-26, same binary, STAMP 20260603T141337Z)

| metric | A (window=0) | B (window=4) | delta |
|---|---|---|---|
| **overall J** | 0.2697 | **0.3882** | **+11.85pp (+44%)** |
| single-hop | 0.031 | 0.156 | +12.5pp (5x) |
| open-domain | 0.077 | 0.308 | +23.1pp (4x) |
| temporal | 0.400 | 0.514 | +11.4pp |
| multi-hop | 0.297 | 0.378 | +8.1pp |
| SEMANTIC-GAP (classify2) | 18 | **13** | -5 converted |
| OK-gold-retrieved | 9 | 11 | +2 |

Smoking-gun q90/q95/q98 ALL flipped SEMANTIC-GAP→OK (cos 0.63/0.60/0.59 →
0.66/0.76/0.74). **This is the FIRST conv-26 retrieval lever that did not falsify**
(ISS-159/164/178/HyDE/MMR all falsified) — because it fixes the data at INGEST,
upstream of retrieval ranking.

### Residual 13 SEMANTIC-GAP (window=4 ceiling)

- q109/q110/q111/q112 — single-session (D8) short-noun gold ("pots","painting"); stored
  correctly but short generic gold keeps embedding-cosine <0.65 = **classify2 measurement
  artifact**, not a real miss (LLM-judge lifted single-hop 5x).
- q33/q45/q49/q76 — gold is a DATE; **temporal-extraction** track (ISS-190/215), not windowing.
- q3 — stored correctly, query↔gold cosine low = **ranking** track.
- q65 — cross-session multi-evidence (D16+D11), referent >4 turns / cross-session = needs
  larger or cross-session window.

### Recommendation

The windowing belongs in the LIBRARY at ingest, not as a bench-driver flag. This is the
concrete first use-case for **ISS-162 ExtractionContext** — thread a bounded ring of
recent turns into `store_raw`/extractor so any conversational ingest (not just LoCoMo)
resolves coref. The bench flag stays as the A/B harness to validate the engramai
implementation against the +11.85pp baseline.

### Artifacts (windowing)

- `engram-bench/src/drivers/locomo.rs:1213` — sliding-window ingest (ENGRAM_BENCH_INGEST_WINDOW)
- `crates/engramai/examples/iss201_window_verify.rs` — LLM-level proof (4/4)
- `/tmp/iss201_window_ab.sh` — A/B harness
- substrate dirs: A=.tmpW2ZHx0 B=.tmpmKeySN (ephemeral); runs ISS201-WIN{A,B}-conv26-20260603T141337Z

## Part 2 re-bench verdict (2026-06-10, run ISS201-P2-conv26-20260610T154234Z)

Single-arm conv-26 under unified new defaults (window-preserve default-on 7af00c6d +
date-pinning 7e0287c2), ISS-218 envelope (K=10, INGEST_WINDOW=4, FACTUAL_REWEIGHT=on).
Compared against ISS218-B-conv26-20260609T155222Z (preserve=on, no pinning).

| metric | ISS218-B (no pin) | P2 (pin) | delta |
|---|---|---|---|
| **overall J** | 0.3289 | 0.3158 | **-1.3pp** |
| multi-hop | 0.243 | 0.324 | +8.1pp |
| open-domain | 0.231 | 0.308 | +7.7pp |
| single-hop | 0.125 | 0.031 | -9.4pp |
| temporal | 0.486 | 0.443 | -4.3pp |

7 gains / 9 losses. **Expected +5-8pp from date-stranding fix did NOT materialize at
bench level** — delta is within known re-ingestion noise (~22/152 flips between
identical-config runs; here 16 flips, cross-ingestion comparison).

### What the pin DID do (q0 evidence)

q0 (gold "7 May 2023") candidate text now reads:
`"[2023-05-08] Caroline feels accepted by the support group ... (2023-05-07)"` —
the resolved day IS in the memory text (pin works, date no longer stranded in note).
But generation still answers IDK with a pedantic refusal: *"this describes her feeling
accepted, not when she went to"*. **Date-stranding converted from an extraction miss
into a generation-conservatism miss.** Several temporal losses (q82/q122/q123/q139/q143)
are re-ingestion churn (q123 lost the guinea-pig memory entirely), not pin regressions.

### Three-bucket sizing on this run (104 misses / 152)

- gold-evidence present in retrieved candidates but wrong/IDK answer
  (**generation/judge bucket**): 35 (23% of all queries) — multi-hop 14, temporal 12,
  single-hop 7, open-domain 2
- gold NOT in candidates (**retrieval/extraction bucket**): 69 (45%) — temporal 27,
  single-hop 24, multi-hop 11, open-domain 7
- 68/104 misses are IDK predictions → generation is over-conservative even when
  evidence is at rank ≤10

### Conclusion

1. Pin fix is correct and kept (dates now surface in memory text) but its score impact
   is gated by generation conservatism — the bucket moved, the score didn't.
2. Generation/judge bucket is now sized at ~23% of queries = up to +23pp headroom.
   Per ranked plan step 3, generation-layer work IS justified (prompt: answer from
   dated evidence even when phrasing differs; stop pedantic entailment refusals).
3. Retrieval/extraction bucket still the largest (45%) — temporal (27) and single-hop
   (24) dominate.
4. Single-run cross-ingestion comparisons are too noisy (±2pp overall, category swings
   ±9pp) to measure <5pp effects; future A/Bs of ingest-side changes need same-DB or
   multi-run protocols.

## Answer-guidance same-pool A/B verdict (2026-06-10, STAMP 20260610T190847Z)

Harness: `ENGRAM_BENCH_ANSWER_GUIDANCE_AB=1` — ingest ONCE, judge each question
TWICE on the byte-identical candidate pool (arm A guidance=off, arm B=on).
Eliminates re-ingestion noise entirely. engram-bench commit `218782b`.

| | arm A (off) | arm B (on) | Δ |
|---|---|---|---|
| overall | 0.3553 | 0.3618 | **+0.66pp** |
| multi-hop | 0.3243 | 0.3514 | +2.7pp |
| open-domain | 0.4615 | 0.5385 | +7.7pp |
| single-hop | 0.0625 | 0.0312 | -3.1pp (1 flip) |
| temporal | 0.4857 | 0.4857 | 0 |

Flips: **4 gains / 3 losses** (152q, same pool — every flip is purely
prompt-induced).

Gains: q81/q88 = IDK→correct (exactly the over-conservatism lever); q63/q74 =
vague-hedge→committed-specific.

Losses: q44 = real regression (guidance pushed commit-to-a-date, picked Aug 12 vs
gold Aug 13 — celebrated-day-before ambiguity); q55/q100 = judge wobble (B answers
arguably equal or better than A).

**q116-style IDK→confident-WRONG risk did NOT materialize: zero such conversions.**

Decision per pre-registered rule (gains > losses): **guidance stays default-on**.
No per-category gating needed. Matches IDK-exemplar probe prediction (~+2-3pp,
not +10pp): guidance only converts misses where gold is already in candidates AND
the failure was pure refusal — a minority of the 35-question generation bucket.
Remaining generation-bucket misses (~31q) are wrong-pick/synthesis, not refusal.

Next lever: Step-2 retrieval autopsy on the 69q unretrieved bucket
(A=ranked-out / B=degraded / C=never-ingested / D=fragmented).

## Step-2 retrieval-miss autopsy (2026-06-10, run ISS201-P2-conv26-20260610T154234Z)

### Input set

Locked retrieval-miss set = **72 qids**: 62 core + 7 disputed-retrieval
(q9/q20/q28/q32/q38/q39/q76) + 3 fuzzy-only (q40/q75/q111). Disputed final
assignment: **generation=28 / retrieval=7** (`/tmp/iss201_disputed_assignment.json`).
Substrate inspected: P2 run's substrate.db (479 memory nodes). Evidence refs
resolved as D{day}:{turn}, 1-based, against the 19-day occurred_at grouping of
conv-26's 419 episodes.

### Method

Classifier `/tmp/iss201_autopsy.py`, per-qid output
`/tmp/iss201_autopsy_results.json`. "Ingested" heuristic = token-overlap ≥0.45
between evidence episode text and best memory (verbatim LIKE unusable — only
111/419 episodes survive verbatim through extraction). Buckets:

- **A ranked-out** — gold fact exists in DB but absent from top-10 candidates
- **B evidence-degraded** — evidence partially ingested, lossy/below threshold
- **C never-ingested** — zero evidence content made it into any memory
- **D fragmented** — gold tokens only covered across ≥2 memories (list answers)

### Spot-check of 6 anomalous qids → 4 manual overrides

- **q123 A confirmed** — "Caroline owns a guinea pig named Oscar" is in DB, not in candidates.
- **q90 C confirmed** — D3:16 "5 years already!" (married 5 years) never extracted.
- **q111 B→A** — precise memory "Melanie and her kids paint together, especially
  nature-inspired paintings (2023-07-08)" exists in DB but unretrieved;
  gold_in_cand=True was a generic-token false positive (bare "painting" at r7/r10).
- **q19 B→D** — both gold facts in DB (dinosaur-exhibit memory + kids-love-nature
  memory), neither retrieved; D-detection missed it because "dinosaurs" ≠ "dinosaur"
  (no stemming).
- **q148 A→C** — its only gold_mem was a false token match ("happy and thankful"
  matched Caroline's LGBTQ-group memory, wrong person/topic); the Grand Canyon
  reaction was never ingested (ev_best 0.19).
- **q143 C→generation-side (removed from set)** — exact memory "Melanie's son got
  into an accident during a roadtrip past weekend; he was okay..." IS in DB
  (ov=0.37, just under the 0.45 threshold) and a related accident memory was
  retrieved at r4; the prediction even cites it but refuses over the "road trip"
  linkage. Pure generation refusal.

Heuristic limitations exposed: (1) no stemming suppresses D detection,
(2) single-token golds make gold_in_cand unreliable, (3) gold-token matching can
hit wrong-person memories, (4) the 0.45 ingestion threshold has edge cases
(q143 at 0.37 was actually a faithful extraction).

### Final tally (n=71 after q143 → generation)

| bucket | n | share |
|---|---|---|
| **A ranked-out** | **33** | **46%** |
| B evidence-degraded | 15 | 21% |
| C never-ingested | 12 | 17% |
| D fragmented | 11 | 15% |

Per-category:

| category | A | B | C | D | n |
|---|---|---|---|---|---|
| single-hop | 11 | 7 | 1 | 10 | 29 |
| temporal | 16 | 6 | 8 | 0 | 30 |
| open-domain | 1 | 2 | 3 | 1 | 7 |
| multi-hop | 5 | 0 | 0 | 0 | 5 |

### Conclusions

1. **A (ranked-out) dominates at ~46%** — the gold memory is in the DB but loses
   the top-10 race. This is a **ranking** problem, not extraction. Next lever:
   **ISS-159 cross-encoder reranker** (CrossEncoderReranker already shipped behind
   feature flag, MiniLM-L-6 @ K_fusion=50) — re-bench it against this run's
   envelope; the 33 A-qids are the direct target population.
2. **D (fragmented, 11q, almost all single-hop list answers)** is the secondary
   lever — extraction splits list facts across memories and top-10 can't cover all
   shards. Candidates: list-aware extraction merging or higher K for list questions.
3. **C (12q)** is irreducible at retrieval time — extractor dropped the fact
   entirely (8 of 12 are temporal; consistent with the date-stranding family,
   ISS-190/191/204 lineage).
4. **B (15q)** = lossy extraction; partially addressable by extraction-quality
   work, but lower priority than A+D.
5. Temporal splits A16/C8 — half ranking, half never-ingested; single-hop is the
   D hotspot (10/11 of all D).

Net: ranking work (cross-encoder rerank over an expanded fusion pool) is the
single biggest lever on the unretrieved bucket, worth up to ~33/152 ≈ 22pp of
ceiling if perfectly fixed (realistically a fraction of that).

## Cross-encoder same-DB A/B verdict (2026-06-10, STAMP 20260610T222430Z)

Follow-up to the Step-2 autopsy: A-bucket (ranked-out) was 33/71 of the
retrieval misses, so we re-tested the ISS-159 cross-encoder (ms-marco-MiniLM-L-6-v2,
k_in=50, post-fusion C.5 hook) **inside the P2 envelope** (conv-26,
INGEST_WINDOW=4, K=10, FACTUAL_REWEIGHT=on, HyDE/MMR/entity off,
PIPELINE_POOL=1). Same-DB harness: one ingestion, retrieval+generation+judge
twice per question (arm A CE-off, arm B CE-on). Run
`ISS159-CE-AB-conv26-20260610T222430Z`, harness commit engram-bench `463cc52`.

### Result: CE WINS (+5.3pp overall, no category regression)

| | arm A (off) | arm B (on) | Δ |
|---|---|---|---|
| overall | 0.3355 | **0.3882** | **+5.3pp** |
| single-hop (n=32) | 0.0938 | 0.1875 | +9.4pp |
| open-domain (n=13) | 0.2308 | 0.3846 | +15.4pp |
| multi-hop (n=37) | 0.2973 | 0.3514 | +5.4pp |
| temporal (n=70) | 0.4857 | 0.5000 | +1.4pp |

18 flips: 13 gains (4 multi-hop, 4 single-hop, 3 temporal, 2 open-domain) vs
5 losses (2 multi-hop, 1 single-hop, 2 temporal). Arm A sanity-matches the
P2 baseline 0.3158 within rejudge wobble (+2pp).

Note this **reverses the ISS-159 2026-05-26 falsification** — that test ran
in the old envelope (HyDE=per_category, MMR=0.7, no FACTUAL_REWEIGHT, no
ingest windowing). The P2 envelope's windowed ingestion + factual reweighting
changed the candidate pool enough that CE now has real signal to work with.

### A-bucket impact: only 3/33 rescued

Of the 33 A-bucket (ranked-out) qids from the Step-2 autopsy, CE flipped
**3 gains, 0 losses**: q4, q20, q141. The other 30 stayed missed. Combined
with q0's flip (gold at rank 1 with CE), the interpretation:

**CE only rescues golds that already reach the k_in=50 fusion pool.** Most
ranked-out golds never enter the pool at all, so no reranker can save them.
The next retrieval lever is therefore upstream of CE: fusion-pool widening
(k_seed/K_fusion 50→100+) and/or per-qid root-cause fixes (embedding misses,
query phrasing, extraction gaps). Per-qid autopsy of the 30 unfixed A-bucket
qids is the immediate next step (potato directive 2026-06-10: skip conv-44
cross-validation, investigate WHY specific golds aren't recalled).

### Recommendation

- Bench envelope: CE **default-on** going forward (P2+CE = new baseline 0.3882).
- engramai lib: CE stays opt-in (feature flag + GraphQuery knob) — no change.
- Artifacts: `benchmarks/runs/ISS159-CE-AB-conv26-20260610T222430Z/`
  (`locomo_ce_ab_diff.json`, per-arm jsonl/summaries), log `/tmp/iss159-ce-ab/master.log`.

## Step-4 pool-dump classification of the 31 unfixed A-bucket qids (2026-06-10)

Run: `ISS201-POOLDUMP-conv26-20260611T012504Z` (P2+CE envelope +
`ENGRAM_BENCH_DUMP_CANDIDATES=1`, dumps in `/tmp/iss201_fused_dumps/`,
275 files). Run overall = 0.4408 vs CE A/B arm-B 0.3882 — cross-ingestion
re-ingest + judge wobble (±2pp overall / ±9pp per-cat documented at P2
re-bench); within-run classification is unaffected.

Classifier: `/tmp/iss201_pool_classify.py` → results
`/tmp/iss201_pool_classify_results.json`. Gold matched against
prefusion/fused `content_head` + final `retrieved_candidates` via
date-substring OR ≥0.8 gold-token coverage (content_head truncates at
~200 chars, full containment too strict).

### Bucket table (n=31)

| bucket | n | qids |
|---|---|---|
| pool-miss (hybrid-anomaly) | 10 | q3 q11 q37 q49 q76 q83 q118 q119 q128 q150 |
| pool-miss (true, gold absent from 100–350-cand prefusion pool) | 8 | q7 q9 q14 q66 q110 q142 q144 q148 |
| CE-ranked-below-top-10 | 9 | q28 q43 q47 q48 q67 q71 q94 q103 q104 |
| in-top-10 (generation/judge-side) | 4 | q82 q85 q106 q123 |

By category: temporal {hybrid:5, ce-below:3, pool-miss:4, in-top10:4},
single-hop {hybrid:4, pool-miss:2, ce-below:4}, multi-hop {pool-miss:1,
ce-below:2, hybrid:1}, open-domain {pool-miss:1}.

### Mechanics

**Hybrid anomaly (10 qids, biggest single bucket).** `PlanKind::Hybrid`
bypasses `fuse_and_rank` entirely (`retrieval/api.rs` ~line 950 match arm),
so (a) `maybe_dump_fused_pool` (`fusion/combiner.rs:536`) never fires —
these qids only have a `prefusion-hybrid` dump — and (b) **the hybrid
candidate pool is genuinely only 10 candidates** (RRF hybrid path), vs
100–350 for Factual-plan queries. Gold absent from a 10-candidate pool is
a real retrieval failure with no CE/fusion recourse downstream.

**CE-below-top-10 (9 qids).** Gold IS in the fused pool but at rank
17–293 (q94=17, q47=83, q28=95, q43=105, q103=127, q48=131, q67=202,
q104=292, q71=293, pool sizes 219–352). CE `k_in` default = **50**
(`fusion/cross_encoder.rs:117`) — 8/9 golds sit beyond the rerank window,
CE never scores them. Only q94 (rank 17) was inside the window and still
lost. This bucket is mostly "CE window too shallow," not "CE wrong."

**True pool-miss (8 qids).** Gold absent even from 100–350-candidate
prefusion pools — vector/BM25 channels never surface it. These need
per-qid root-cause (embedding miss, extraction gap, paraphrase distance);
pool-widening alone may not reach them. Caveat: q9/q66/q142/q144/q148
show partial coverage 0.25–0.67, some may be content_head-truncation
false negatives.

**In-top-10 (4 qids).** Gold text reaches the final top-10; q82/q123
actually scored 1.0 this run (re-judge flips), q85/q106 are
generation/judge misses, not retrieval.

Note: q37/q14/q142 scored 1.0 despite gold-absent-from-pool
classification — judge leniency / paraphrase acceptance; their "unfixed"
status is stale per-run.

### Lever recommendation (ordered by coverage)

1. **Widen the Hybrid plan's candidate pool 10 → 50+** (api.rs Hybrid
   match arm / RRF path). Covers 10 qids — the largest bucket — incl. 4
   temporal + 4 single-hop. Also add a fused-dump hook to the Hybrid arm
   for observability.
2. **Deepen the CE rerank window** `k_in` 50 → 200–350 (or full pool).
   Covers up to 8 of the 9 ce-below qids whose golds CE currently never
   sees. Latency: ~1.5ms/pair × 300 ≈ 0.45s/query — acceptable for bench.
   Must pair with checking *why* fusion ranks golds at 80–300 (BM25/vector
   score starvation), since CE quality on deep candidates is unproven.
3. **Per-qid root-cause for the 8 true pool-misses** — pure
   k_seed/K_fusion widening (50→100) helps at most these 8, and several
   look like extraction/paraphrase gaps rather than pool-size issues.
4. In-top-10 bucket (4) belongs to the generation/judge track, not
   retrieval.

Levers 1+2 together address 19/31 (61%) of the unfixed A-bucket misses.

## Lever 1 + Lever 2 results (2026-06-11)

Both levers from the Step-4 recommendation were implemented and benched
serially on conv-26 (P2 envelope + CE=1, fresh ingestion per run).

### Lever 1 — Hybrid pool 10 → 50 (committed `8253f478`)

- `retrieval/orchestrator.rs` ~1451: Hybrid plan `top_k = query.limit.max(50)`
- `retrieval/api.rs` Hybrid arm: added `maybe_dump_fused_pool` hook
  (observability parity with fuse_and_rank path)
- Run `ISS201-LEVER1-conv26-20260611T035733Z`: **overall 0.4342**
  (single-hop 0.25, multi 0.324, open 0.385, temporal 0.586)
- vs POOLDUMP 0.4408 = flat (within ±2pp re-ingest wobble) → **no regression,
  committed**. All 10 hybrid-anomaly qids now produce 50-candidate fused
  dumps.
- Per-qid: q3/q37/q150 flipped to 1.0. q3/q11 golds entered pool at rank
  33–34 (CE k_in=50 didn't lift them — handed to lever 2).
  q49/q76/q83/q118/q119/q128 golds **still absent from the 50-pool** —
  these are fusion/ingest gaps, not pool-cap, reassigned to the
  true-pool-miss family.

### Lever 2 — CE k_in 50 → 250 (env `ENGRAM_BENCH_CROSS_ENCODER_K_IN`)

- engram-bench `locomo.rs:807` env override; no engramai code change
  (`CrossEncoderConfig.k_in` was already configurable).
- Run `ISS201-LEVER2-conv26-20260611T043611Z`: **overall 0.5197 — new best**
  - single-hop **0.4375** (+18.75pp vs lever-1; the weakest category
    nearly doubled)
  - temporal 0.671, open 0.462, multi 0.324
  - vs lever-1 0.4342 = **+8.6pp**, far beyond ±2pp noise
  - vs ISS201-P2 baseline 0.3158 = +20.4pp cumulative
- Flips vs lever-1: **20 gains / 7 losses, net +13**.
- Target ce-below qids: q43/q47/q103 → 1.0. q28/q67 moved from IDK to
  concrete-but-wrong dates (gold-adjacent content now in top-10 →
  reclassified generation-side). q3 regressed (k_in=250 introduced
  competing candidates that displaced lever-1's rank-33 CE lift).

**Decision: `ENGRAM_BENCH_CROSS_ENCODER_K_IN=250` becomes the bench
default envelope going forward.** The CE finally consumes the deep
candidate pool it was designed for (ISS-159); k_in=50 was the binding
constraint all along.

### Residual misses after lever 1+2 (next lever = PPR)

- **Ranked-out residue** (q3/q11 family): gold in pool, CE can't
  discriminate at depth → structural ranking fix needed, not more depth.
- **True pool-miss** (q49/q76/q83/q118/q119/q128 + original 8): gold never
  surfaces in any channel — extraction/paraphrase/fusion-seed gaps.
- Generation bucket unchanged (~35q): IDK over-caution + entailment
  refusals.

Next structural lever: **PPR ablation arm** (HippoRAG2-style Personalized
PageRank over the unified entity+memory node graph, replacing BFS 1-hop +
fixed-weight fusion in the Associative/Hybrid channels). Filed separately
— see ISS-221. **Prerequisite: ISS-203 entity canonicalization** (PPR on a
fragmented entity graph measures the fragmentation, not the algorithm).

---

## Pure bi-encoder recall probe (2026-06-13) — confirms retrieval is NOT the bottleneck

Question raised: "is the per-memory recall hit-rate under 50%?" (J-score ~0.53 read literally as recall). Ran the `iss186_candidate_pool_probe` on **all 152 conv-26 questions** (4 categories, top-200, pure cosine — bypasses fusion / CE / MMR / HyDE / plan classifier). Out: `/tmp/recall_probe/conv26-all-20260613T034146Z.jsonl`. Gold matching = substring + date-normalized + content-word-subset, with evidence-episode fallback.

### Recall@K (pure bi-encoder ceiling)

| category | n | recall@10 | recall@50 | recall@200 |
|---|---|---|---|---|
| multi-hop | 37 | **0.568** | 0.811 | **0.919** |
| temporal | 70 | 0.414 | 0.443 | 0.471 |
| single-hop | 32 | 0.094 | 0.188 | 0.312 |
| open-domain | 13 | 0.000 | 0.000 | 0.000 |
| ALL | 152 | 0.349 | 0.441 | 0.507 |

### The 73 "MISS" are measurement false-negatives, not real recall failures

Bucketed the 73 D-bucket (gold not string-matched in top-200) by gold type, then **hand-verified the 7 "short-fact" + spot-checked others** against their actual top-5 recalled text:

- **list/aggregate — 43 (59%)**: comma-joined answers ("pottery, camping, painting, swimming"). No single memory carries the whole list verbatim; the relevant memories ARE recalled.
- **inference — 6 + count/duration — 4 (14%)**: derived answers ("Likely no", "5 years", "once or twice a year"). Source memories recalled to top-5; answer requires reasoning, not a literal string.
- **short-fact — 7**: hand-checked — all relevant memories recalled, missed only on surface form. e.g. q43 gold "abstract art" → r2 "created an **abstract painting**"; q86 gold "LGBTQ+ individuals" → r2 "helps **LGBTQ+** folks with adoption"; q11 gold "Sweden" → r1 "moving from her home country" (country name deeper). Zero genuine recall failures in this set.
- **other — 13**: long-sentence golds, same pattern.

**Genuine "relevant memory absent from top-200" cases ≈ 0.** Concrete-fact recall is healthy (multi-hop @200=0.92; temporal/single-hop relevant memories consistently in top-5 — verified q82/q90/q117/q132/q11/q43/q86/q140).

### Verdict — independent confirmation of this issue's thesis

The original premise ("memory recall hit-rate <50%") is **false**. The J-score deficit lives entirely downstream of recall, in exactly the layers this issue names:

1. **Ranking** — gold recalled to top-200 but not top-10 (multi-hop @10=0.57 vs @200=0.92 ⇒ ~35pp lost to ranking). → ISS-159 cross-encoder deepening, k_in tuning.
2. **Multi-hop / aggregation** — list answers require assembling items scattered across several memories.
3. **Generation over-caution** — facts present in candidates (marriage date, beach mentions), model refuses to compute "date − date = 5 years" / "multiple mentions → once or twice a year", answers IDK. → ISS-201 answer-guidance track.

**Retrieval (bi-encoder recall) is not the bottleneck and the embedder does not need replacing.** Next real levers = ranking (CE) + generation (answer-from-evidence prompting), per the Step-2 autopsy. Probe artifacts: `/tmp/recall_probe/` (jsonl + analyse.py).

---

## Follow-up lever (2026-06-13) — k_seed starvation: CE k_in=250 has been running on a 10-candidate pool

The recall probe above proved gold reaches bi-encoder top-50 (multi-hop @50=0.81) but final answers miss. Traced the pipeline to find why CE can't fix it:

- **`orchestrator.rs:1455`**: `with_k_seed(query.k_seed_override.unwrap_or(query.limit))` — the fusion seed pool defaults to `query.limit` (=K=10), NOT the budget.rs default of 10-per-channel. Only widened if `k_seed_override` is set.
- **`cross_encoder.rs:272`**: CE reranks `candidates[..k_in]`. LEVER2 sets `k_in=250`.
- **But no LEVER2 / ISS-223 run ever set `ENGRAM_BENCH_K_SEED`** → k_seed stayed = limit = 10 → fusion fed CE only ~10 candidates → **CE's k_in=250 was empty capacity. CE was reshuffling the 10 gold-poor candidates fusion already picked, never seeing the top-11..50 gold the bi-encoder recalled.**

This is exactly what `api.rs:203` warns ("k_seed=limit too narrow to surface specific-fact evidence episodes, widen k_seed") and what ISS-159's residual ("CE can't save gold not in the fusion pool") pointed at — now root-caused with hard recall data.

The bench already has the knobs (`ENGRAM_BENCH_K_SEED` + `ENGRAM_BENCH_BM25_POOL`, locomo.rs:581/592 — note both must lift together). They were simply never used alongside CE.

**Experiment (independent arms, pool-level change so same-DB A/B not applicable):**
- Arm A: LEVER2 status quo (k_seed=10, CE k_in=250 = empty capacity)
- Arm B: K_SEED=250 + BM25_POOL=250 + CE k_in=250 (pool matches CE appetite; CE finally sees recalled gold)

Hypothesis: B lifts single-hop/multi-hop meaningfully (the 35pp ranking gap the recall probe exposed). If B flat → the gap is generation-side not ranking, pivot to answer-guidance. Tracking run STAMP added on launch.

### k_seed A/B result (2026-06-13) — FALSIFIED, ranking is NOT the bottleneck

Ran the independent A/B (runs `ISS201-KSEED-{A,B}-conv26-20260613T043653Z`):

| arm | overall | single-hop | multi-hop | open-domain | temporal |
|---|---|---|---|---|---|
| A (k_seed=10, status quo) | 0.5132 | 0.375 | 0.3514 | 0.4615 | 0.6714 |
| B (K_SEED=250 + BM25_POOL=250) | 0.5000 | 0.375 | 0.3514 | 0.4615 | 0.6429 |

**Zero gain; overall −1.3pp (within ±2pp ingestion noise + temporal −2.9pp).** single-hop / multi-hop / open-domain byte-identical.

**Correction to the starvation hypothesis above:** instrumenting Arm A's pool depth (`grep candidates= arm-A.log`) showed most queries already carry **200–300 candidates** — only 24 queries hit candidates=50. The factual plan's `memories_mentioning_entity` channel is NOT k_seed-limited; CE k_in=250 was already reranking a deep pool on most queries, NOT empty capacity. Widening k_seed to 250 therefore fed CE the same gold it already saw → no reorder change.

### Bottleneck localized by elimination → GENERATION layer

Three independent measurements now converge:
1. Recall probe: gold reaches bi-encoder top-50 (multi-hop @50=0.81). Recall ✓ not the bottleneck.
2. Pool depth: CE reranks 200–300 candidates; gold is in-pool. Pool ✓ not the bottleneck.
3. k_seed A/B: widening the pool yields zero gain. Ranking ✓ not the bottleneck.

The only remaining layer is **generation**: the model receives candidates containing the gold (top-200, post-CE likely top-10) but answers wrong/IDK because it won't perform secondary reasoning (date subtraction, count aggregation, inference). This is exactly thesis-layer 3 (generation over-caution).

**Next lever = answer-guidance (prompt the model to reason from dated/counted evidence), NOT ranking, NOT recall, NOT embedder swap.** Do not re-tune k_seed — falsified here.

### answer-guidance A/B result (2026-06-13) — MARGINAL/INERT, within noise

Ran the same-DB A/B (`ENGRAM_BENCH_ANSWER_GUIDANCE_AB`, run `ISS201-GUIDANCE-AB-conv26-20260610T190847Z`, ingest once / judge twice, LEVER2 envelope):

| arm | overall | single-hop | multi-hop | open-domain |
|---|---|---|---|---|
| A (guidance OFF) | 0.3553 | 0.0625 | 0.324 | 0.462 |
| B (guidance ON)  | 0.3618 | 0.0313 | 0.351 | 0.538 |

**Net +0.66pp overall = inside the ±2pp cross-ingestion noise floor.** Flip ledger: **4 gains, 3 losses = net +1 question.**

- Gains (0→1): q63/q74 (multi-hop, date phrasing nudge), **q81/q88 (IDK→committed inference — clean wins, guidance working as designed)**.
- Losses (1→0): q44 (B emitted wrong date Aug-12 vs gold Aug-13 — date extraction, not guidance), q55 (sunset/sunrise judge wobble), q100 (pure judge wobble — both answers semantically equivalent).

**Single-hop floored in BOTH arms (2/32 vs 1/32).** The guidance cannot move single-hop because single-hop misses are **retrieval/extraction surface-form** problems (the gold fact never surfaces in an answerable form), not generation-reasoning. Guidance only helps multi-hop/open-domain, where the model needs *permission to infer* — and even there the net is judge-wobble-sized.

**Verdict:** answer-guidance is a marginal lever. Keep gated OFF by default (preserves ISS-100 mem0-parity envelope); opt-in only. It converts real IDK→answer cases but induces equal-magnitude judge wobble, so the net sits in the noise.

### Updated bottleneck map (all three layers now measured)

| layer | lever tried | result |
|---|---|---|
| recall | bi-encoder top-50 probe | gold @50 = 0.81 — recall ✓ not the bottleneck |
| ranking | k_seed 10→250 A/B | zero gain — ranking ✓ not the bottleneck |
| ranking | MMR λ<1.0 (ISS-223) | falsified on xval — not the lever |
| generation | answer-guidance A/B | +0.66pp, within noise — marginal |

**The residual deficit concentrates in SINGLE-HOP, which is floored at ~3–6% and is immune to recall-widening, ranking-widening, and prompting.** This points the next investigation at the single-hop **extraction surface-form**: the gold fact is recalled (in the candidate pool) but stored in a form the generator cannot map to the question's surface (e.g. dated event with date stranded out of text, possessive/phrase-entity fragmentation, atomic fact buried in a multi-clause summary). That is an *extraction/representation* problem, not retrieval, ranking, or generation-prompting. Candidate next levers: (a) atomic single-fact extraction (one fact per memory), (b) date-into-text pinning (already partly done ISS-190/191/204 — verify it reaches single-hop golds), (c) entity-mention canonicalization (ISS-203) so single-hop anchor resolution lands on the right node.

### single-hop extraction autopsy (2026-06-13) — DECISIVE REFRAME: it's a LIST problem

Ran an evidence-grounded autopsy on the 30 floored single-hop misses (arm A of run `ISS201-GUIDANCE-AB-conv26-20260610T190847Z`), joining fixture questions+gold+evidence with the per-query `retrieved_candidates` and a per-item top-200 recall probe. Scripts archived to `artifacts/`.

**Bucket tally (n=30):**

| bucket | n | meaning |
|---|---|---|
| LIST-MISS | 11 | gold is a multi-item set; <50% of items in the K=10 pool |
| LIST-PARTIAL | 9 | gold is a set; ≥50% items in pool but incomplete → gen undercounts |
| ATOMIC-MISS | 7 | genuine single deep-fact, absent from pool |
| ATOMIC-INPOOL | 2 | fact present, gen/judge surface mismatch |
| DATE-STRAND | 1 | date stripped from text |

**Headline: 20/30 (67%) are LIST/SET questions** ("what hobbies", "which pets", "what instruments", "which books"). The "single-hop" label is a misnomer — these require gathering scattered items across many memories into one set.

**Root cause (verified via per-item top-200 recall probe):** list items are SCATTERED across retrieval ranks, not co-located. Examples:

- q18 (camping spots = beach, mountains, forest): mountain @rank 2, forest @6, **beach @15** → beach falls outside K=10 → incomplete list → judge=0
- q60 (instruments = clarinet, violin): clarinet @3, **violin @77**
- q15 (activities = pottery, camping, painting, swimming): camping @149, swimming @69
- q66 (marshmallow @12, stories @67)
- q52 (pets Oliver/Luna/Bailey): all @≤3 — but the *atomic memory* doesn't exist; "Oliver" only appears as an aside inside a cat-anecdote memory, never as a clean "Melanie's pets are Oliver, Luna, Bailey"

**Every list item exists in the source conversation** (hand-verified against episodes) — this is NOT a storage drop. It is a **co-location + window** problem: to score a list answer the generator needs ALL items co-present in top-K=10, but they rank at 2/6/15/77/149 so only a partial set fits.

**Two compounding defects:**
1. **No atomic aggregate memory.** Extraction never consolidates "Melanie camped at: beach, mountains, forest" into a single memory; each item lives in its own episode ranking independently.
2. **K=10 window too small to hold a scattered set.**

Secondary (smaller): ~6% of extracted memories are pure interrogative turns ("X asked Y what…") carrying no answerable fact, +~11% backchannel/dialogue noise polluting the pool.

**ATOMIC-MISS (7)** are the only true deep-fact misses: q3 (adoption agencies), q4 (Transgender woman), q7 (Single), q11 (Sweden), q71 (Becoming Nicole), q75 (3 kids) — single facts stated once, not surfacing.

**Why the earlier elimination chain masked this:** recall was measured as whole-pool bag-of-words hit-rate, which counts each list item as "recalled" because each is individually in top-200. But list answers need the items JOINTLY in top-10 — they never co-occur. recall✓/ranking✓ were both true *for atomic queries* and false *for list queries*, which the aggregate metric averaged away.

**Next levers (data-pointed):**
- **(a) list-aware retrieval** — detect enumeration/list intent ("what/which … s") → widen K dramatically OR do per-item sub-retrieval then union the results. Cheapest test.
- **(b) aggregate-memory extraction** — synthesize set memories ("person's hobbies: X, Y, Z") during consolidation so the whole set is one high-ranking candidate.
- **(c) interrogative-turn filter** — stop the extractor emitting question-only/backchannel memories that crowd the K=10 window.

### lever-(a) list-aware retrieval — global-K probe (2026-06-13): CONFIRMED, biggest lever yet

Ran the blunt global-K A/B (runs `ISS201-LISTK-{A,B}-conv26-20260613T060852Z`): Arm A K=10 (status quo) vs Arm B K=30 (wider window), LEVER2 envelope, only `ENGRAM_BENCH_TOP_K` differs.

| metric | A (K=10) | B (K=30) | Δ |
|---|---|---|---|
| overall | 0.5066 | **0.5855** | **+7.9pp** |
| single-hop | 0.3438 | **0.5625** | **+21.9pp** |
| multi-hop | 0.3514 | 0.3784 | +2.7pp |
| open-domain | 0.4615 | 0.5385 | +7.7pp |
| temporal | 0.6714 | 0.7143 | +4.3pp |

**+7.9pp overall is far beyond the ±2pp cross-ingestion noise floor.** Single-hop +21.9pp is the largest single lever found in the entire ISS-201 campaign.

**Per-bucket flip analysis (the precise signal, not the noisy overall):**
- **LIST (20 ids): A_won=8 → B_won=13 = net +5 questions correct.** Gains: q15 (activities), q24 (destress methods), q34 (events), q38 (6-item hobbies list), q39 (LGBTQ activities), q52 (pet names Oliver/Luna/Bailey). One loss: q51 (judge wobble).
- **ATOMIC guard (10 ids): A_won=3 → B_won=4, ZERO regressions** — even gained q3 (adoption agencies). **Wider K does NOT hurt atomic/precise queries.** This kills the "more candidates = more noise = worse precision" concern.

**Root cause confirmed:** the scattered list items the autopsy mapped (beach@15, violin@77, swimming@69) now fit inside the K=30 window, so the generator can assemble complete sets and pass the binary judge.

**Open questions before shipping:**
1. **conv-44 cross-validation** — is K=30 a conv-26 overfit, or corpus-general? (MMR λ=0.5 looked good on conv-26 then died on conv-44 — must not repeat that mistake.) Run the same A/B on conv-44 next.
2. **global K=30 vs targeted list-aware K** — given ZERO atomic regression, global K=30 may simply be strictly better (no need to detect list intent). But it inflates generator context cost on the ~80% non-list queries. Decide after conv-44: if the lift replicates with no regression, ship global K=30; if atomic regresses on conv-44, build the targeted detector (enumeration-intent → widen K only for list questions).

### lever-(a) conv-44 cross-validation (2026-06-13): FALSIFIED as corpus-general — K=30 was conv-26 overfit

Ran the identical global-K A/B on conv-44 (inverted ratio 13 list / 17 atomic, harder atomic guard set). Runs `ISS201-LISTK44-{A,B}-conv44-20260613T073119Z`, same LEVER2 envelope, only `ENGRAM_BENCH_TOP_K` differs (A=10, B=30). conv-44 fixture = 675 ep, 123 q, sha 4a87346d.

| metric | A (K=10) | B (K=30) | Δ |
|---|---|---|---|
| overall | 0.5366 | 0.5366 | **+0.0pp** |
| single-hop | 0.5667 | 0.5667 | +0.0pp |
| multi-hop | 0.3333 | 0.2917 | −4.2pp |
| open-domain | 0.1429 | 0.0000 | −14.3pp |
| temporal | 0.6452 | 0.6774 | +3.2pp |

**Per-bucket flip analysis (the decisive signal):**
- **LIST (13 ids): A_won=5 → B_won=6 = net +1.** Single gain q31, zero losses. +1 on n=13 is within judge/ingestion noise — NOT the net+5 conv-26 produced.
- **ATOMIC guard (17 ids): A_won=12 → B_won=11 = net −1.** Loss: q48 (A=1→B=0). **The atomic guard REGRESSED.** On conv-26 the guard had zero regression; here wider K demoted a previously-correct atomic answer below the cut.

**Verdict: K=30 is a conv-26 overfit. Both falsification conditions in the pre-committed decision rule fire:**
1. LIST flat (net +1 ≈ noise, not the conv-26 net+5)
2. ATOMIC guard regresses (−1)

This is exactly the MMR λ=0.5 trap — a knob that looks like the biggest lever of the campaign on conv-26, then evaporates (and mildly harms) on the cross-validation corpus. The +21.9pp single-hop / +7.9pp overall conv-26 lift does NOT replicate: conv-44 single-hop and overall are byte-flat. The conv-26 gains were specific to how conv-26's list items happened to scatter into ranks 11–30; conv-44's list items do not sit in that band, and the extra 20 candidates only add noise that costs an atomic question.

**Why the divergence:** conv-26 is list-heavy (21 list / 9 atomic) and its scattered list items clustered in the 11–30 rank band — K=30 swept them in. conv-44 is atomic-heavy (13 list / 17 atomic); its list items are either already in top-10 or scattered beyond rank 30, so widening to 30 gains almost no lists but adds 20 distractors that occasionally outrank a precise atomic gold. Global wider-K is therefore not corpus-general.

**Decision — PIVOT to lever (b) aggregate-memory extraction.** A window widening cannot solve item scatter that exceeds the window; the robust fix is to make the *set* a single high-ranking candidate at ingestion/consolidation time (synthesize "person's hobbies: X, Y, Z" memories), so list answers retrieve as one unit regardless of where individual mentions scatter. This also avoids the per-query generator context-cost inflation that global K=30 would impose on the ~80% non-list queries.

A *targeted* enumeration-intent detector (widen K only for list questions) is NOT worth building given conv-44 shows the lift doesn't even hold for lists on a second corpus — the scatter-beyond-window failure mode is what kills it, and a targeted K=30 would hit the same wall. Lever (b) is the next concrete step; lever (c) interrogative-turn filter remains a cheap parallel cleanup.

---

## lever-(b) DESIGN: entity-centric aggregate-memory synthesis (2026-06-13)

### Problem this fixes
List/set questions ("what are X's hobbies/pets/instruments") fail because the answer items scatter across retrieval ranks and never co-locate in top-K. conv-44 proved a window-widening (global K) fix is a per-corpus overfit. The robust fix: make each *set* a single high-ranking candidate at consolidation time, so a list answer retrieves as ONE memory.

### Substrate reality (probed on conv-44 Arm B substrate, 2026-06-13)
The enumerable sets EXIST in the graph but are NOT separable by deterministic grouping:
- Central person "Audrey" = 528 outgoing structural edges. Predicate dist: `related_to` 216, `uses` 131, `occurred_on` 93, `leads_to` 33, `depends_on` 32, `part_of` 14.
- The pet-set {Pepper, Precious, Panda} AND the hobby-set {hiking, bird-watching, nature} BOTH live inside the single flat `related_to` bag of 216 targets — interleaved with pure noise ("photo", "girlfriend", "apartment", "making memories", "furry friend").
- Canonical predicate vocab (IsA/PartOf/WorksAt/Uses/RelatedTo/OccurredOn/...) has NO fine-grained relations (`plays_instrument`, `has_pet`, `has_hobby`). Across the whole corpus `related_to` is 494 of all structural edges — the dominant fallback.

**Therefore a naive `GROUP BY (subject, predicate)` produces one giant `related_to` blob per person, not clean answerable sets.** Aggregation MUST be an LLM clustering pass, not SQL.

### Why the existing synthesis subsystem can't be reused as-is
`synthesis/engine.rs` clusters by *embedding-graph Infomap* (topic similarity) and `synthesis/insight.rs` builds prompts that explicitly demand a "NEW observation / pattern / rule, NOT a summary" and forbid enumeration. That is the opposite of what a set-memory needs (exhaustive enumeration of one attribute of one entity). Reusing the cluster→LLM plumbing is fine; the *clustering key* and the *prompt* must both change.

### Design

**Clustering key = subject entity (not embedding topic).**
1. New consolidation sub-pass `synthesize_entity_sets` (runs inside `sleep_cycle` / `consolidate`, after dirty-cluster synthesis).
2. Candidate selection: pick entity nodes whose outgoing structural-edge count ≥ threshold (e.g. ≥ 6) AND that have ≥ N (e.g. ≥ 3) distinct object targets — these are the entities that *could* answer a list question. Skip low-degree entities (no set to form).
3. For each candidate entity: gather all `(predicate, object_text, source_memory_id)` outgoing edges (cap at a budget, e.g. top 60 by edge weight/recency to bound the prompt).
4. **LLM bucketing call** (Haiku): given entity name + the flat object list, return JSON of typed attribute-sets, e.g. `{"pets":["Pepper","Precious","Panda"], "hobbies":["hiking","bird-watching"], ...}`, dropping noise/non-set items. The prompt forces: only group items that are genuinely co-members of one attribute; discard relational/event/possessive fragments; do not invent items not present.
5. For each returned set with ≥ 2 members, emit (or UPDATE) one **set-memory**: content = canonical surface form ("Audrey's pets: Pepper, Precious, Panda"), `memory_type=Factual`, high importance, provenance edges to every contributing source_memory_id, and a `set_memory` marker in attributes so it is idempotently re-synthesizable (UPDATE not duplicate on re-run).

**Idempotency / drift:** key the set-memory on `(entity_id, attribute_label)`. On re-consolidation, re-bucket and diff: add new members, keep existing, mark removed only if the source edge was invalidated. Mirrors the existing insight `clusters_auto_updated` path.

**Retrieval impact:** set-memory is a normal memory, so it enters the same FTS + vector pools. A list question ("what are Audrey's pets") will embed close to "Audrey's pets: ..." and retrieve the WHOLE set at rank ~0 — solving the co-location problem at the source instead of fighting it with K.

### A/B plan (must cross-validate, per the K=30 lesson)
- Flag: `ENGRAM_BENCH_ENTITY_SETS` (default off) gating `synthesize_entity_sets`.
- Arm A = off, Arm B = on. Same LEVER2 envelope.
- Run BOTH conv-26 AND conv-44 before any ship decision (the K=30 trap: conv-26-only looked great then died on conv-44).
- Primary signal = per-bucket flip on the same LIST id sets already classified (conv-26: q15,18,19,23,24,32,34,38,39,47,48,51,52,56,60,61,65,66,70,78; conv-44: q2,3,9,12,21,23,29,31,35,36,41,51,54). Guard = ATOMIC ids must not regress (a wrong set-memory could pollute atomic retrieval).
- Ship rule: LIST net-positive on BOTH corpora AND ATOMIC no-regress on BOTH → ship. Otherwise iterate the bucketing prompt or abandon.

### Open risks
- **Noise leakage:** if the LLM buckets "girlfriend"/"apartment" into a spurious set, the set-memory becomes a confident distractor that could cost atomic questions (the q48-style regression seen with K=30). The prompt's discard-rule and the ATOMIC guard are the defense; watch the guard closely.
- **Entity fragmentation (ISS-203):** "Caroline" vs "caroline" vs "Caroline's art" split the edges across nodes, so a set may be incomplete. Lever-(b) benefits from but does not require ISS-203 canonicalization; note the interaction.
- **Cost:** one extra Haiku call per high-degree entity per consolidation. Bounded by the degree threshold (only ~handful of central entities per conversation).

### Next concrete step
Implement `synthesize_entity_sets` behind the flag + the bucketing prompt, then run the conv-26 + conv-44 A/B. Do NOT ship on a single corpus.

### lever-(b) IMPLEMENTATION integration points (verified 2026-06-13)

Confirmed the bench DOES build the full graph before queries, so set-memories can be synthesized in-band:

- **bench callsite:** `engram-bench/src/drivers/locomo.rs` `replay_conversation`, immediately AFTER `memory.shutdown_pipeline(...)` (line ~1445, which drains the resolution pipeline so `graph_entities`/edges are fully populated) and BEFORE the gold-question query loop (line ~1467). Gate on `ENGRAM_BENCH_ENTITY_SETS`. This is the same structural slot ISS-106's reverted `compile_knowledge` used — but lever-(b) clusters by ENTITY not embedding-topic, so it does NOT hit the single-super-cluster degeneration that killed ISS-106.
- **engram callsite (production parity):** new `Memory::synthesize_entity_sets(namespace)` also wired as "Phase 2b" in `sleep_cycle` (memory.rs ~7107, after the existing `synthesize()` Phase 2), gated by a `SynthesisSettings.entity_sets` flag so production consolidation gets the same behavior, not just the bench.
- **emit pattern to mirror:** the existing insight path in `synthesis/engine.rs` (transaction boundary, provenance edges, `clusters_auto_updated` idempotent-update logic) — reuse the storage/provenance plumbing, replace the clustering key (entity, not Infomap) and the prompt (enumerate, not abstract).

ISS-106 cautionary note carried in locomo.rs already documents WHY a naive embedding-topic compile regressed; lever-(b)'s entity-keyed clustering is the deliberate avoidance of that failure mode.

---

## lever-(b) FALSIFIED on conv-26 (2026-06-13)

Implemented (engram `3570c119`, engram-bench `9147e13`) and A/B'd on conv-26.
Run `ISS201-ESET-{A,B}-conv26-20260613T091147Z`, LEVER2 envelope, only
`ENGRAM_BENCH_ENTITY_SETS` toggled.

**Synthesis fired cleanly:** `considered=20 bucketed=20 written=22 updated=0
llm_calls=20 errors=0` — 20 high-degree entities bucketed by Haiku with zero
parse/LLM errors, 22 set-memories written to the pool. The mechanism works.

**But the verdict is negative on the primary signal:**

| metric | A (off) | B (on) | Δ |
|---|---|---|---|
| overall | 0.5066 | 0.5197 | +1.3pp (within ±2pp re-ingest noise) |
| single-hop | 0.4062 | 0.3125 | **−9.4pp** |
| multi-hop | 0.3514 | 0.3784 | +2.7pp |
| temporal | 0.6286 | 0.700 | +7.1pp |
| **LIST(20)** | A_won=9 | B_won=6 | **net −3** |
| ATOMIC(10) | A_won=3 | B_won=3 | net 0 |

LIST gains: q51. LIST losses: q18, q19, q24, q52. **Ship rule (LIST net>0)
FAILS.** conv-44 NOT launched — falsified on its home corpus.

### Root cause: the set-memory is a LOSSY, HIGH-RANKING DISTRACTOR

The hypothesis was "items scatter, never co-locate, so synthesize one complete
memory." The opposite happened. Per-query autopsy of the 4 LIST losses (all the
same pattern):

- **q52 (Melanie's pets, gold "Oliver, Luna, Bailey" — the canonical lever-(b)
  target):** A enumerated all three from scattered fact-memories ("Luna (dog),
  Oliver (cat), Bailey (cat)") → correct. B answered "two cats Oliver and
  Bailey" → **dropped Luna**, wrong. The synthesized pets-set bucketed only the
  cats (mis-typed / lost a member), ranked high, and the generator anchored on
  it INSTEAD of enumerating from the raw facts.
- **q18 (camping spots, gold "beach, mountains, forest"):** A got all three; B
  dropped "beach".
- **q24 (destress, gold "Running, pottery"):** A listed 5 incl. both; B
  narrowed and the judge marked it 0.
- **q19 (kids' likes, gold "dinosaurs, nature"):** both verbose, B's bucket
  re-shaped the list away from the gold tokens.

**The individual fact-memories were already retrieving well enough to
enumerate.** The set-memory did not ADD the missing item — it REPLACED the rich
scattered evidence with a lossy bucket and then crowded out the raw facts (high
rank as a single dense candidate). The LLM-bucketing pass is itself lossy
(member-dropping, mis-typing two-cats-vs-three-pets) and becomes a confident
distractor — exactly the q48-style failure mode the ATOMIC guard was watching
for, but it landed on LIST instead.

### Decision

- **lever-(b) abandoned for retrieval/LoCoMo.** The premise (scatter beyond
  top-K) does not hold on conv-26 the way the single-hop autopsy implied — the
  facts ARE in top-K; the deficit is generation enumeration-completeness, not
  retrieval co-location. A synthesized set can only match or under-perform the
  raw enumeration, and in practice under-performs because bucketing is lossy.
- **Code kept inert** (flag default off, byte-identical arm A) — the storage
  helpers + module are sound and unit/round-trip tested; no revert needed.
- **Real bug fixed in passing:** `upsert_set_memory`'s FTS refresh used the
  FTS5 `('delete', rowid, '')` special command (requires exact prior content,
  else leaves the row → same-rowid reinsert collides 1555). Now uses the
  canonical `DELETE ... WHERE rowid` idiom. Worth keeping regardless.
- **Pivot:** the consistent signal across BOTH lever-(a) (K=30) and lever-(b) is
  that LIST/enumeration answers are **generation-completeness** limited, not
  retrieval-limited — A's verbose multi-item answers beat B's narrowed ones.
  Next lever = generation-side: prompt the answerer to enumerate ALL matching
  items from the evidence (anti-narrowing guidance), reusing the
  `ENGRAM_BENCH_ANSWER_GUIDANCE` same-pool A/B harness. This is cheap (no
  re-ingest, judge-twice on one pool) and directly targets the observed loss
  mode (B dropping members the evidence contained).

---

## lever-(a) global-K widening — CROSS-VALIDATION FALSIFIED + Hybrid-pool premise corrected (2026-06-13)

Investigated the compaction-flagged "widen the Hybrid candidate pool (capped
~10, bypasses fuse_and_rank)" lever. Two corrections + a decisive cross-val.

### The Hybrid-pool premise was STALE

Re-read `crates/engramai/src/retrieval/{orchestrator.rs,api.rs,plans/hybrid.rs}`:

- The ~10 truncate cap **no longer exists** — the Hybrid arm already sets RRF
  `top_k = query.limit.max(50)` (ISS-201 lever-1, previously committed).
- Hybrid bypassing `fuse_and_rank` is **by design** (§5.2): RRF already produced
  a fused score; re-fusing would double-count. Hybrid candidates still flow
  through the **cross-encoder + MMR at Stage C.5** like every other plan.
- q51/q52's `prefusion-hybrid` dumps show 10 candidates because the **sub-plans
  themselves only emitted 10** — pool starvation is at the sub-plan retrieval
  source, not at a truncation cap. There is **no Hybrid-cap code fix to make.**
- q51 gold member "horse painting" is genuinely **absent from the pool** (only
  sunset/sunrise present) — corpus/pool-completeness limited, not rankable.

### KSEED A/B (cross-encoder seed pool k_seed 10→250) — NULL RESULT

`ISS201-KSEED-{A,B}-conv26-20260613T043653Z`: overall 0.513 → 0.500 (flat),
**every category unchanged**. Gold candidates are NOT sitting in ranks 11-250
waiting for the CE — deeper reranking / seed-pool widening buys nothing. The
bottleneck is not CE seed starvation.

### LISTK A/B (final TOP_K 10→30) — conv-26 looked great…

`ISS201-LISTK-{A,B}-conv26-20260613T060852Z`: overall +7.9pp (0.507→0.586),
**single-hop +21.9pp** (0.344→0.563), LIST bucket **net +5** (gains
q15/q24/q34/q38/q39/q52, one wobble-loss q51), ATOMIC guard **zero regression**
(+1, q3). Because KSEED-deeper was flat, this lift is **generation-side** — the
generator simply sees more candidates in context and enumerates more list
members.

### LISTK44 conv-44 cross-validation — FALSIFIES global K=30

`ISS201-LISTK44-{A,B}-conv44-20260613T073119Z` (same K=10→K=30 change):
overall **FLAT** (0.5366 = 0.5366, 8 gains / 8 losses pure churn), LIST net
**+1** (q31 only; noise on n=13), ATOMIC guard **net −1 (q48 regressed)**.

conv-44 has the inverted ratio (13 list / 17 atomic) and stresses the atomic
guard harder — and the guard **broke**. Global K=30 is **conv-26 overfit**: it
trades a home-corpus single-hop win for conv-44 atomic regression with no
generalizable net gain.

### Decision

- **Do NOT ship global K=30.** Falsified on cross-validation.
- **No Hybrid-pool code fix** — the cap was already removed; the dumps that
  motivated the lever predate lever-1.
- **Confirmed lever across (a)+(b)+KSEED:** the conv-26 LIST/single-hop deficit
  is **generation-completeness + pool-completeness** limited, NOT
  retrieval-rank or CE-seed limited.
- **Candidate next lever (un-launched):** *targeted* list-intent K widening —
  detect enumeration intent and widen **only the final top-K passed to the
  generator** for list questions (leave atomic at K=10). This could capture the
  conv-26 LIST gains without the conv-44 atomic regression, because the regress
  source (atomic questions seeing more distractors) is excluded by construction.
  Requires an enumeration-intent detector + a per-query generation-boundary K
  override, and must be cross-validated on BOTH corpora before shipping.

---

## Correction + targeted-K lever ALSO killed (2026-06-13, deeper conv-44 read)

On closer per-query inspection the "conv-44 atomic guard broke" claim was
**wrong** — and the correction kills the *targeted* list-K lever too.

### The conv-44 "regression" was judge wobble, not distractors

- **q48** (single-hop, gold "Family"): A(K10) "friends, confidantes, and
  **family** members" → 1.0; B(K30) "friends and confidantes who listen,
  provide comfort" → 0.0. **Same memory, B just omitted the word "family".**
  Judge wobble on an equivalent answer, not a distractor-induced retrieval miss.
- Of the 16 conv-44 flips, **9 are temporal** (cross-ingestion judge noise) plus
  multi-hop wobble. There is **no coherent atomic-degradation signal** — the
  premise behind "exclude atomics and the regression goes away" is unfounded.

### The real conv-44 LIST result: K=30 does nothing for the hard cases

LIST bucket (n=13): A=5.0 → B=6.0, a **single** gain (q31). The 7 hard zeros
stayed **0.0 at both K=10 and K=30**. Inspecting their predictions shows the
failure is **pool/extraction completeness, not final-K sizing**:

- **q3** (gold: cafes, eateries, hike-space, pet shelter, wine, park):
  generator enumerated cafes/parks/wine — missed 3 members entirely.
- **q12** (gold: rock climbing, fishing, camping): got rock climbing, then
  hallucinated "kayaking, bungee jumping" — fishing/camping **not in pool**.
- **q2** (gold: boardgames, volunteering, wine, growing flowers): got 3 of 4 —
  **"growing flowers" member pool-absent**.

A candidate that is never retrieved (or never extracted into a memory) cannot
be enumerated no matter how wide K is. That is exactly why K=30 was flat on
conv-44's hard list questions.

### Decision — the entire "widen the pool / widen K" family is closed

- Global K=30: conv-26 overfit (falsified).
- **Targeted list-intent K (generation-boundary override): NOT WORTH BUILDING.**
  Its only justification was "exclude atomics to dodge the conv-44 regression"
  — but that regression was judge wobble, and widening K did **nothing** for the
  hard list cases on conv-44. The lever would add an intent detector + per-query
  K plumbing to capture, at best, the few already-borderline conv-26 questions
  that K=30 flipped — with no benefit on the genuinely-hard cases.
- **Root cause of the hard list deficit = pool-completeness + extraction**, not
  retrieval-rank, not CE-seed, not final-K. The members are missing from the
  candidate set (scatter beyond any reasonable window) or were never extracted
  into memories. **The next real lever must target retrieval recall / extraction
  coverage of list members — not K sizing.**

---

## Member-level recall diagnostic — the deficit is TWO problems (2026-06-13)

Ran the pure bi-encoder top-200 recall probe (`iss186_candidate_pool_probe`,
bypasses fusion/CE/MMR) on conv-26 all-152q, then checked **each gold list
member's rank** individually (the aggregate `gold_hits` matcher requires ALL
members in one memory, which unfairly D-buckets list questions — member-level
is the correct lens).

### Aggregate recall ceiling (pure bi-encoder, top-200)

- single-hop recall@200 = **0.312** (68% gold MISSED beyond top-200)
- open-domain recall@200 = **0.000** (gold NEVER in top-200 for any of 13)
- temporal recall@200 = 0.471 · multi-hop = 0.919 · **ALL = 0.507**

Half of all gold evidence is absent from the top-200 **before any reranking**.
This is a recall ceiling, not a ranking problem.

### Member-level split of the LIST bucket

**Problem 1 — RANK SCATTER (members present, spread beyond K=10):**
- q15: pottery@58 camping@149 painting@15 swimming@69 — **4/4 present**
- q18: beach@15 mountains@2 forest@6 — **3/3 present**
- q24: running@1 pottery@74 — **2/2 present**
- q38: 5/6 present, ranks 14-143

These members ARE retrievable — just scattered far beyond K=10. A wider final-K
or better fusion-diversity **does** help them (exactly why conv-26 K=30 flipped
q15/q24/q34/q38/q39).

**Problem 2 — EXTRACTION ABSENCE (member not in top-200 at all):**
- q34: Mentoring program + school speech — **0/2 present**
- q39: all four members — **0/4 present**
- q19: dinosaurs absent · q48: bowls absent · q32: school speech absent
- q23: "Nothing is Impossible" absent

These members were never extracted into a memory (or embed too far from the
query). **No K-widening recovers them.**

### Why this explains the K=30 conv-26-vs-conv-44 divergence

conv-26's flippable list questions are mostly **Problem 1** (scatter) → K=30
caught them. conv-44's hard list zeros are mostly **Problem 2** (extraction
absence) → K=30 was flat. Global K=30 isn't "overfit" so much as it only ever
addresses scatter, and the two corpora have different scatter/absence ratios.

### Revised lever map

1. **Problem 1 (scatter)** — addressable by fusion-diversity (MMR is already
   wired, λ-tunable) or a *list-intent* final-K bump. The earlier "targeted-K
   not worth building" call stands ONLY because the gains are small and
   corpus-dependent; if pursued, pair it with MMR so the wider window adds
   diverse members rather than near-duplicates of rank-1.
2. **Problem 2 (extraction absence)** — the bigger, more general frontier.
   Members like "mentoring program", "school speech", "dinosaurs", "bowls" never
   made it into a retrievable memory. Next step: trace the extractor on the
   conv-26 episodes that *state* these facts — are multi-item / incidental
   mentions being dropped or collapsed during extraction? This is a NEW issue
   (extraction recall), distinct from ISS-201's retrieval-side scope.

---

## Source-episode trace: most "extraction absence" is gold-grounding / phrasing, not extraction (2026-06-13)

Traced the Problem-2 absent members back to the raw conv-26 episodes
(`benchmarks/fixtures/locomo/.../conversations.jsonl`, 419 episodes). The
"extraction absence" bucket is mostly **not** an extractor bug:

- **q19** ("What do Melanie's kids like?", gold "dinosaurs, nature") — episode
  97: *"They were stoked for the dinosaur exhibit! They love learning about
  animals."* The token "dinosaur" exists, but it ranks **>200** because the
  episode is a one-time **event** ("exhibit visit"), while the question asks for
  a **preference**. Query↔document phrasing asymmetry, not extraction loss.
- **q34** ("What events has Caroline participated in to help children?", gold
  "Mentoring program, school speech") — **"school speech" appears 0× in all 419
  episodes.** "mentor" appears 7× but only as people who *support* Caroline
  ("support from friends and mentors", "my friends, family and mentors are my
  rocks"). **No episode states Caroline ran a mentoring program or gave a school
  speech.** The gold is essentially **unsupported by the conversation.**

### Implication for the achievable ceiling

A meaningful fraction of conv-26 single-hop/list "misses" are either
**ungroundable** (gold not in corpus) or **recall-limited by event-vs-preference
asymmetry**. The achievable J-score ceiling on conv-26 is **below 100%**, which
partly explains the persistent ~0.50–0.53 plateau independent of ranking work.

### Final lever map (closing this investigation thread)

- ❌ Hybrid-pool widening — already done / by-design, no fix exists.
- ❌ CE seed-pool deepening (KSEED) — flat, not the bottleneck.
- ❌ Global K=30 — conv-26-only; only ever helps Problem-1 scatter.
- ❌ Targeted list-intent K — premise (atomic-regression dodge) was bogus; only
  helps scatter, and even then corpus-dependently.
- ✅ **Problem-1 SCATTER** (members at ranks 15-149) — best addressed by
  **MMR-diversity tuning** (λ already wired) so the existing window surfaces
  *diverse* members instead of rank-1 near-dupes. Cheap, bounded, no new plumbing.
- ✅ **Query↔document asymmetry** subset of Problem-2 — addressable by **query
  expansion / HyDE** (event→preference bridging) — but ISS-141/HyDE was already
  explored; revisit only as a query-layer concern.
- ⛔ **Ungroundable gold** (q34 "school speech") — not fixable; it caps the
  ceiling and should be excluded from any "list recall" target metric.

**ISS-201 retrieval-side levers are exhausted.** Remaining upside is
MMR-diversity for scatter + accepting a sub-1.0 grounded ceiling. Extraction
recall is a separate concern only for the genuine event-vs-preference subset.

---

## temporal + IDK-拒答 桶诊断 (2026-06-13)

底座 `ISS222-LEVER2-conv26-20260612T002433Z` (overall **0.5329**, vector channel
已修)，对失败做了结构化分桶。

### 失败结构

- **IDK 拒答失败：36 条** — temporal 18 / single-hop 9 / open-domain 5 / multi-hop 4
- **temporal 失败 25/70** — 其中 **18 条是 IDK 拒答**，仅 7 条是真答错

### 关键修正：temporal-IDK 几乎全是检索问题，不是生成端可修的 prompt 问题

之前 q0-root-cause 的假设是"证据到手却被生成器拒答"。把 18 条 temporal-IDK
用今天的 bi-encoder top-200 recall 探针交叉后：

- **13 条 gold 完全召回不到 top-200** → 纯检索/grounding。
- **5 条看似在 top-10**（q93/q94/q104/q106/q128）—— 但逐条看生成输出，它们
  **也是检索问题**：探针的模糊匹配命中了**同一 entity 的别的记忆**，而不是 gold
  那条。例：
  - q93（问 grandma 送的礼物，gold "necklace"）：生成器召回到"朋友送的手绘碗"
    那条，诚实回答"没提 grandma 的礼物"。
  - q94（gold "art and self-expression"）：召回到 Melanie 的碗/画，但**没召回到**
    那条明说"碗让我想起 art and self-expression"的记忆（即 ep62 Caroline 那条）。
  - q104（gold ""Becoming Nicole""）：召回到"Melanie 在读 Caroline 推荐的书"，
    但书名那条没召回。

**生成器的 IDK 在这些 case 里是诚实且正确的** —— 它手上确实没有 gold 那条记忆。
"生成端拒答 → 改 prompt" 这个可修子桶**基本不存在**。

### 结论（与 list 诊断一致，殊途同归）

temporal-IDK 桶 ≈ 全是 **recall/grounding** 瓶颈。具体两个检索难点：
1. **date-stranding** — 事件记忆召回到了，但文本不带日期；temporal 查询匹配不上。
2. **同-entity-多记忆选错** — 召回到了正确的人/物，但具体那条 gold 记忆没进 top-10。

这跟 list 诊断的结论收敛到同一处：**真正的前沿是检索召回 + grounding，不是排序、
不是 K-sizing、不是生成 prompt。** ISS-201 retrieval-side 至此彻底封板；下一步
应转向 recall/extraction 的独立 issue（date-grounding + entity-记忆消歧）。
