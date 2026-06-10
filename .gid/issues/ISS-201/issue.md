---
title: "conv-26 LoCoMo failure decomposition: 3 distinct layers (retrieval short-circuit / generation over-caution / judge wobble), NOT a single bottleneck"
status: open
priority: P1
severity: major
labels: [v04-unified-substrate, locomo, diagnosis, retrieval, generation]
feature: v04-unified-substrate
created: 2026-05-31
relates_to: [ISS-198, ISS-186, ISS-149, ISS-161, ISS-179, ISS-188]
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
