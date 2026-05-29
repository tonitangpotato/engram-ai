---
title: 'Candidate pool diagnostic — where is gold memory ranked for 27 conv-26 SF queries (A=top-10 / B=top-50 / C=top-200 / D=missing)'
status: in_progress
priority: P0
severity: diagnostic
category: retrieval-foundation
created: 2026-05-28
relates: [engram:ISS-148, engram:ISS-149, engram:ISS-159, engram:ISS-164, engram:ISS-175, engram:ISS-178, engram:ISS-179, engram:ISS-181]
discovered_in: 2026-05-28 potato session — 9 retrieval-side levers falsified in one week
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

- [ ] AC-1: `engram-bench/examples/iss186_candidate_pool_probe.rs`
  exists, takes conv-26 substrate path + question file,
  emits JSONL with top-200 per SF query.
- [ ] AC-2: `/tmp/iss186_analyse.py` (or
  `engram-bench/examples/iss186_analyse.rs`) reads JSONL,
  recovers gold memory rank, buckets, outputs aggregate
  table.
- [ ] AC-3: Per-query table + aggregate bucket counts
  written to `.gid/issues/ISS-186/artifacts/conv26-buckets-{STAMP}.md`.
- [ ] AC-4: Decision section in this issue body picks
  one of {A-heavy, B-heavy, C-heavy, D-heavy} next-move
  paths, citing the data.

## Why P0

This unblocks ISS-179 AC-5a redefine, ISS-148 (single-hop
≥0.40 target), and every retrieval-side lever issue. Without
this data we keep guessing.

## Status

In progress 2026-05-28 — probe binary + analyse script
being written.
