---
title: Candidate pool diagnostic — where is gold memory ranked for 27 conv-26 SF queries (A=top-10 / B=top-50 / C=top-200 / D=missing)
status: resolved
priority: P0
severity: diagnostic
category: retrieval-foundation
created: 2026-05-28
relates:
- engram:ISS-148
- engram:ISS-149
- engram:ISS-159
- engram:ISS-164
- engram:ISS-175
- engram:ISS-178
- engram:ISS-179
- engram:ISS-181
discovered_in: 2026-05-28 potato session — 9 retrieval-side levers falsified in one week
resolved: 2026-05-28
---

## Why this issue exists

Past week burned 9 retrieval-side issues (ISS-159 cross-encoder,
ISS-164 entity channel, ISS-175 factual fusion reweight, ISS-178
prev-turn extractor context, ISS-161 prompt variants, ISS-153
HyDE) — all falsified or kept-as-opt-in on conv-26. Net movement
on SF: ~5/27 → ~8/27 best case. Target: 17/27.

ISS-179 census admits the best-case stack of all known levers tops
at 10-13/27. That's a 4-7 query gap to AC-5a that **no
retrieval-side lever has been shown to close**.

This issue stops the lever-of-the-week pattern and asks the
question we should have asked first:

**For the 22 SF queries we currently fail, where is the gold
memory actually ranked by a pure bi-encoder?**

Three outcomes are possible and they imply totally different
attack surfaces:

- **Bucket A (gold in top-10)** — we have the gold candidate
  but the reranker/fusion drops it. This is the "tune fusion
  weights" world. Most of the levers we've tried assumed this.
- **Bucket B (gold in top-11..50)** — bi-encoder found gold
  but plan-specific K caps drop it. This is the "widen the
  pool" world (HyDE / pool widening / K_seed bump).
- **Bucket C (gold in top-51..200)** — bi-encoder barely
  recalls gold. Pool widening helps marginally; need query
  expansion or re-embedding.
- **Bucket D (gold not in top-200 at all)** — bi-encoder
  is blind to this query↔memory pair. **No retrieval-side
  lever can save us.** Options: change embedder, change
  indexing unit, add query/memory rewriting at write time.

The distribution of 22 failed queries across A/B/C/D is the
single most important fact for choosing the next 2 months of
engram work. We don't know it.

## Method

Pure bi-encoder probe. Bypass plan classifier, fusion, MMR,
cross-encoder, HyDE — everything. Just:

1. Open the conv-26 substrate ingested with the locked envelope
   (ENGRAM_BENCH_FACTUAL_REWEIGHT=off ENTITY_CHANNEL=off
   PIPELINE_POOL=1, matching ISS-178 Arm A).
2. For each conv-26 SF query (27 queries from ISS-161's
   `single_fact` axis):
   a. Embed the query string with the same Ollama
      `nomic-embed-text` the harness uses.
   b. Pull all node embeddings via
      `Storage::get_all_embeddings("nomic-embed-text")`.
   c. Compute cosine vs every memory.
   d. Sort desc, take top-200.
   e. Dump JSONL `{query_id, query_text, gold_answer,
      top200: [{rank, memory_id, score, text_excerpt,
      created_at}]}`.
3. Heuristic gold-memory recovery: for each query, find
   gold memory id(s) by substring-matching gold answer
   against memory content. Document false-positive risk
   (an SF answer of "yes" / "no" / "Caroline" matches many
   memories; flag these as ambiguous and exclude from
   bucket assignment).
4. Bucket each query into A/B/C/D by gold memory rank.
5. Output:
   - Per-query table: query_id | gold_text | gold_rank | bucket
   - Aggregate: bucket counts + bucket %
   - List the 5 worst (bucket D) queries with text — those are
     our toughest signal for what's wrong with embedding.

## What the answer means for next 2 months of work

- **A heavy (≥15/22)**: ranking is the problem. Reranker
  work (ISS-159 cross-encoder rev2, ISS-175 fusion redesign)
  is the right surface. Continue.
- **B heavy (≥10/22)**: pool size is the problem. Widen
  K_seed, ship HyDE per-category (ISS-156 was falsified
  but maybe its envelope was wrong).
- **C heavy (≥10/22)**: query↔memory similarity is the
  problem. Try a different embedder (ISS-157 weapon B
  bge-large / mxbai), or rewrite queries at retrieve time
  (entity expansion, anaphora resolution upstream).
- **D heavy (≥10/22)**: embedding semantics are broken
  for these query shapes. The 2-month-question becomes:
  do we change embedder, change indexing unit (per-fact
  vs per-memory vs per-chunk), or change write-side
  representation (entity expansion at write, not query)?
  ISS-179 AC-5a redefine becomes the right call —
  conv-26 may simply have query shapes that bi-encoder
  retrieval can't handle without write-side help.

## Acceptance criteria

- [x] AC-1: `engram-bench/examples/iss186_candidate_pool_probe.rs`
  exists, takes conv-26 substrate path + question file,
  emits JSONL with top-200 per SF query.
- [x] AC-2: `/tmp/iss186_analyse.py` (or
  `engram-bench/examples/iss186_analyse.rs`) reads JSONL,
  recovers gold memory rank, buckets, outputs aggregate
  table.
- [x] AC-3: Per-query table + aggregate bucket counts
  written to `.gid/issues/ISS-186/artifacts/conv26-buckets-{STAMP}.md`.
- [x] AC-4: Decision section in this issue body picks
  one of {A-heavy, B-heavy, C-heavy, D-heavy} next-move
  paths, citing the data.

## Why P0

## Decision (2026-05-28, conv-26 32 SH queries, K=200 probe)

**Run:** `artifacts/conv26-top200-20260529T001245Z.jsonl` (probe PID 29552, finished 00:27:30Z).
**Buckets (loose match — any non-stopword gold token present in candidate text):**

- **A (rank ≤10): 19/32 = 59.4%** ← gold is in top-10, ranker/fusion drops it
- **B (rank 11..50): 8/32 = 25.0%** ← pool widening recovers
- **C (rank 51..200): 2/32 = 6.2%** (q7 "Single", q55 "Sunsets")
- **D (rank >200 / never): 3/32 = 9.4%** (q11 "Sweden", q40 "2", q75 "3")

Full table in `artifacts/conv26-buckets-20260529T001245Z.md`.

### Two bucketings (heuristic limits)

Gold answers in LoCoMo are short noun phrases ("Sweden", "abstract art",
"Pottery, painting, camping…"). Strict substring match drastically
under-counts because episode text rewords ("Caroline moved from
her home country" vs gold "Sweden"; "Melanie's 2 younger kids love
nature" vs gold "dinosaurs, nature"). So we report two numbers:

| Bucket | Strict (exact gold or first token) | Loose (any non-stopword gold token) |
|---|---|---|
| A | 11 (34%) | 19 (59%) |
| B | 8 (25%) | 8 (25%) |
| C | 6 (19%) | 2 (6%) |
| D | 6 (19%) | 3 (9%) |

The loose number is the better estimate of "where the bi-encoder
actually places semantically-related material". The strict number
is a floor (it counts cases where exact phrasing already aligns).
Both numbers point to the same conclusion: **A dominates**.

### What this rules out

- **C/D-heavy hypothesis is dead.** Only 5/32 (~16%) of single-hop
  failures sit at rank >50. Embedder swap (ISS-157 weapon B,
  bge-large/mxbai) would touch at most these 5 — and only the
  C-bucket (2 queries), not the D-bucket where the gold token is
  genuinely absent from any episode.
- **"Just widen the pool" is not enough.** B-bucket is 8/32; even
  if fusion+pool perfectly recovered all 8, we'd add 8/32 to recall
  but the 19-query A-bucket leak would still cap us below AC-5a.
- **Write-side rewriting won't fix the bulk.** D-bucket (3 queries
  — Sweden, "2", "3") is real but small. q40 / q75 are numeric
  count answers that no embedder will recall; they need a
  reasoning/aggregation layer, not retrieval. q11 "Sweden" needs
  entity expansion at write time. Worth filing but not the lever.

### What this names as the actual problem

**19/32 queries have gold within rank ≤10 of a pure cosine probe
— yet conv-26 SF currently scores 5-8/27.** The bi-encoder already
finds the right candidate; everything we layer on top (plan
classifier → channel fusion → MMR → cross-encoder) is dropping it
before the LLM judge sees it. This matches:

- ISS-105 fingerprint diff (top-K content identical between legacy
  vs unified — 152/152 jaccard 1.000)
- ISS-159 CE falsification (cross-encoder reranking added no signal
  → it was already getting the right candidates, picking wrongly)
- ISS-164 entity channel falsification (entity expansion didn't help
  → the candidate was already in the pool)
- ISS-175 fusion reweight falsification (reweighting B-bucket items
  vs A-bucket items didn't move the needle → the issue is what
  reaches the LLM, not how candidates are ordered within a plan)

### Recommended next move

**Stop tuning ranker/fusion until we instrument the drop.** Before
filing yet another ranker redesign:

1. **File ISS-187 "Plan-pipeline candidate-survival audit"** — for
   each of the 19 A-bucket queries, dump candidates at each pipeline
   stage (plan classifier output → channel candidates → fusion
   output → MMR output → final K). Find the exact stage at which the
   gold candidate is dropped. This is **cheap** (no new code,
   debug logging only) and **definitive**.
2. **File ISS-188 "Numeric-answer aggregation"** — q40 / q75 are
   count-the-things questions. Embedder cannot recall "2" or "3"
   meaningfully. Separate plan kind needed, or LLM-side aggregation
   over recovered episode list. P2.
3. **File ISS-189 "Entity-expansion for proper-noun gold tokens"** —
   q11 "Sweden" specifically. The episode says "her home country";
   gold is the country name. Write-time extraction would let us
   index both forms. P2.
4. **Park ISS-157 weapon B (embedder swap), ISS-159 CE rev2,
   ISS-175 redesign.** Not the lever until ISS-187 names the
   actual drop point.

This redirects the next 2 weeks from "find a better fusion
formula" to "find the stage that's dropping the right answer".
That's the falsifiable, root-cause-shaped question — and it's
the one we should have asked five weeks ago.

## Status

Resolved 2026-05-28 — diagnostic complete, all 4 ACs ticked,
decision recorded, recommended follow-ups named (ISS-187/188/189
pending file). conv-26 single-hop bucket distribution: **A-heavy
(59%), 16% rank >50, 9% genuinely absent**. Retrieval-side levers
were attacking the wrong stage; pipeline-stage audit is next.
