---
title: Factual plan ranking floor — 75% routing + 0.063 single-hop pre-channel, Factual subplan ranks worse than Hybrid fallback
status: open
priority: P1
severity: ranking-floor-too-low
category: retrieval
created: 2026-05-27
relates:
- ISS-148
- ISS-164
- ISS-171
discovered_in: ISS-171 AC-6 sweep (STAMP 20260527T112718Z, 2026-05-27 morning)
blocked_by: ''
relates_to: engram:ISS-171
blocks:
- engram:ISS-164
- engram:ISS-148
---

## Summary

After ISS-171 wired `GraphEntityLookup` and unlocked Factual
routing (114/152 conv-26 queries now route to Factual, up from
0/152), aggregate scores **regressed** vs the pre-ISS-171
baseline that ran Associative-by-default:

| metric        | pre-ISS-171 (Hybrid/Assoc fallback) | post-ISS-171 Arm A (channel=off) | Δ        |
|---------------|--------------------------------------|----------------------------------|----------|
| overall       | 0.362                                | 0.204                            | −15.8pp  |
| single-hop    | 0.219                                | 0.063                            | −15.6pp  |
| multi-hop     | 0.351                                | 0.189                            | −16.2pp  |
| open-domain   | 0.385                                | 0.231                            | −15.4pp  |
| temporal      | 0.443                                | 0.271                            | −17.2pp  |

The fix is architecturally correct (Factual is now reachable as
designed in §3.1). But the Factual plan's *internal ranking* of
its own candidates is worse than the Hybrid/Associative pipelines
those queries were silently falling back to.

## Code-layer evidence

Same STAMP, both arms:

```
plan_kind histogram (Arm A and Arm B, identical):
  114 factual
   30 hybrid
    7 associative
    1 abstract
```

Per-query log lines show Factual *retrieves* plenty of
candidates:

```
[INFO] execute_plan ENTER plan_kind=factual query_limit=10 ...
[INFO] execute_plan EXIT  plan_kind=factual candidates=141 outcome=ok
[INFO] execute_plan ENTER plan_kind=factual query_limit=10 ...
[INFO] execute_plan EXIT  plan_kind=factual candidates=180 outcome=ok
```

Candidate pools of 100–180 per query, exiting `outcome=ok`. So
the plan runs, retrieves, doesn't error — but the top-10
returned to the LLM judge are wrong substantially more often
than what Hybrid was returning pre-ISS-171.

## Why this matters

ISS-148 AC-5a (single-fact ≥ 0.60) was the original target.
ISS-164 entity_channel was supposed to be the lever; ISS-171
unblocked the prerequisite (classifier routing). With both
landed:

- ISS-164 Phase 2 RE-RERUN (post-ISS-171) Arm A: SF 0/9
- ISS-164 Phase 2 RE-RERUN (post-ISS-171) Arm B: SF 1/9 (q37 flipped)
- single-hop category Δ A→B: +3 (2 → 5 out of 32)
- multi-hop Δ A→B: +1 (7 → 8 out of 37)
- open-domain Δ A→B: 0 (3 → 3 out of 13)

The entity_channel anchors ARE measurable (+3 single-hop, +1 SF
on the ISS-161 set), but they're rescuing the Factual plan from
a much lower floor than the pre-ISS-171 baseline. We can't ship
the channel based on +3 single-hop because the floor regression
of −15pp swallows the gain.

This is now the actual ISS-148 AC-5a bottleneck. Anchor work
(ISS-164/165/166/167/171) is done; the remaining gap is in how
Factual plan turns 141–180 candidates into a top-10 list.

## Three hypotheses

### H1 — Ranker mismatch (most likely)

Factual plan uses a different ranker / fusion than Associative.
If Factual is pure embedding-cosine over BM25-retrieved
candidates and Associative is BM25-then-embedding-rerank (or has
MMR/diversity baked in), the same gold passage gets surfaced
differently. Pre-ISS-171, every query ran Associative's
ranker; post-ISS-171, 75% run Factual's, and Factual's ranker
is provably worse on LoCoMo-shaped questions.

**Probe**: dump Factual plan's pre-fusion candidate scores vs
Hybrid's on the same 9 single-fact queries. Compare which
passages are ranked top-10 by each path. If Hybrid would have
ranked the gold passage in top-10 but Factual ranks it #50, the
ranker is the bug.

### H2 — Entity-seed expansion drowns gold

If Factual plan over-expands from the resolved anchor (Caroline
→ all 200 episodes mentioning Caroline → ranked by some
non-gold-friendly heuristic), the gold passage is one of 200
and the top-10 returned to the LLM is dominated by
not-the-fact-being-asked. Pre-ISS-171 routing through Hybrid
fused entity-mentions with semantic similarity, which
self-corrects.

**Probe**: for q3 ("What did Caroline research?"), check
whether the gold passage is in Factual plan's pre-fusion
candidate pool at all. If yes, where in the ranking? If it's
in pool but ranked #50–#150, it's an expansion-drowning bug.

### H3 — Missing FTS/BM25 hookup inside Factual

If Factual plan's `hybrid_sub_plan(Factual)` only does graph
traversal + embedding similarity but doesn't run lexical
matching (BM25 against the question text), it'd miss
lexical-match gold passages that Hybrid catches. The
"hybrid_sub_plan" name suggests it should be hybrid, but the
sub_kind=Factual specifically may be skipping the FTS leg.

**Probe**: read `crates/engramai/src/retrieval/plans/factual.rs`
(or wherever `hybrid_sub_plan` lives) and check whether
sub_kind=Factual runs BM25/FTS. If it doesn't, that's a clear
missing channel.

## Acceptance criteria

- [ ] **AC-1**: Locate the ranker — read Factual plan
  implementation, identify the candidate ranking + truncation
  step (which heuristic produces the top-10).
- [ ] **AC-2**: Determine which hypothesis applies (H1 ranker
  mismatch / H2 over-expansion / H3 missing FTS).
- [ ] **AC-3**: For each of the 9 ISS-161 SF queries, dump
  whether the gold passage is in Factual's pre-fusion pool;
  if yes, what rank.
- [ ] **AC-4**: For the same 9 queries, dump where the gold
  would be ranked by the Hybrid path. The diff is the size of
  the regression.
- [ ] **AC-5**: Propose + ship a fix (rebalance ranker, cap
  expansion, add FTS leg — depending on H1/H2/H3 outcome).
- [ ] **AC-6**: Re-run ISS-164 Phase 2 A/B sweep on conv-26
  with the Factual plan fix. Decision rule:
  - overall ≥ 0.34 (within ±2pp of pre-ISS-171 baseline 0.362)
    AND single-fact lift from entity_channel ≥ +2 → ship both,
    reopen ISS-164 with corroborated verdict;
  - overall ≥ 0.34 AND SF lift < +2 → ship Factual fix only,
    leave entity_channel locked-off;
  - overall < 0.34 → root-cause not addressed, file follow-up.

## Out of scope

- Reverting ISS-171 — the wiring is architecturally correct,
  the bug is in what we wired into.
- Rewriting the classifier or signal thresholds — orthogonal.
  Factual routing is correct; Factual *retrieval/ranking* is
  the problem.
- ISS-162 (extraction context) and ISS-163 (semantic UPDATE) —
  blocked by this issue. They were queued behind ISS-164 which
  is itself behind this.

## Artifacts

- Sweep STAMP: `20260527T112718Z`
- Arm A log: `/tmp/iss164-bench/iss164-A.log`
- Arm B log: `/tmp/iss164-bench/iss164-B.log`
- Master: `/tmp/iss171-ac6-sweep-master.log`
- Arm A per-query: `engram-bench/benchmarks/runs/ISS164-A-conv26-20260527T112718Z/locomo_per_query.jsonl`
- Arm B per-query: `engram-bench/benchmarks/runs/ISS164-B-conv26-20260527T112718Z/locomo_per_query.jsonl`
- Arm A summary: `overall=0.204 single-hop=0.063 multi-hop=0.189 open-domain=0.231 temporal=0.271`
- Arm B summary: `overall=0.230 single-hop=0.156 multi-hop=0.216 open-domain=0.231 temporal=0.271`

## Estimated effort

3–5 days. AC-1/AC-2/AC-3/AC-4 are read-only probes (1 day).
Fix depends on hypothesis (1–3 days). Re-run sweep (~1h wall).

## Why this didn't show up earlier

Pre-ISS-171, the classifier was hardcoded to NullEntityLookup
→ every query routed to Associative. The Factual plan code path
existed and was unit-tested but never received a single
production query in any LoCoMo run, ever. ISS-171's AC-6 smoke
test is what surfaced this — first time Factual plan was
exercised on real LoCoMo conv-26 queries, and its top-10 is
demonstrably worse than what Associative would have returned.

## Suggested first move

```
grep -n "fn execute" crates/engramai/src/retrieval/plans/factual.rs
grep -n "hybrid_sub_plan" crates/engramai/src/retrieval/plans/
```

then for q3 from the bench:

```
ENGRAM_BENCH_DUMP_CANDIDATES=1 ENGRAM_BENCH_QUERY_FILTER=conv-26-q3 \
  ./target/release/engram-bench locomo ...
```

inspect candidate pool + ranks. ~30 minutes, no API spend, points
directly at H1 vs H2 vs H3.

## 2026-05-27 09:35 — AC-1/AC-2 verdict from code-layer probe

**Root cause confirmed: H1 + H3 fused. Factual plan never emits `vector_score`.**

Signal table from `crates/engramai/src/retrieval/orchestrator.rs::factual_to_scored`
(line 357) and sibling functions:

| Plan         | vector_score | graph_score      | bm25_score | recency |
|--------------|--------------|------------------|------------|---------|
| **Factual**  | **None**     | anchor-fraction  | FTS        | none    |
| Associative  | seed_score   | 1 / 2^edge_hops  | FTS        | none    |
| Episodic     | none         | none             | FTS        | yes     |
| Affective    | text_score   | none             | FTS        | yes     |

`factual_to_scored` (line ~393) emits only:
```rust
let sub_scores = SubScores {
    graph_score: Some(graph_score.clamp(0.0, 1.0)),   // anchor-fraction
    bm25_score:  Some(bm25),                           // ISS-147
    ..Default::default()                               // vector_score = None
};
```

Where `graph_score = seen_via.len() / total_anchors` — every memory
that mentions any anchor gets at least 1/N; memories that mention
ALL anchors get 1.0. For "What did Caroline research?" with 1 anchor
(Caroline), **every Caroline-mentioning memory has graph_score = 1.0**.
That gives 100–180 tied candidates and only BM25 (lexical) is left
to distinguish them. Embedding similarity to the question text — the
strongest semantic signal — is **never computed**.

This is the H1+H3 combined bug:
- **H1 ranker mismatch**: Associative emits `vector_score` from its
  seed_recaller (provider.embed(query.text)). Factual emits nothing.
- **H3 missing semantic leg**: same as H1 from a different angle —
  there's no per-candidate cosine(query_embedding, memory_embedding)
  pass in the Factual scoring stage.

H2 (over-expansion) is ALSO true (100–180 candidates per query) but
it's a *consequence* of H1+H3: the count would be fine if there were
a strong ranker to surface the gold passage. The flat graph_score
across all candidates is what makes the over-large pool catastrophic.

## Wiring inspection (where the fix goes)

`factual_to_scored` already calls `loader.load_embeddings(&id_strs)`
at line 380 (ISS-139 Strategy A — MMR diversity hook). So
`embeddings_by_id: HashMap<&str, Vec<f32>>` is already in scope per
candidate. What's missing is the **query embedding**.

`PlanCollaborators` (line 92) carries `entity_resolver`,
`episodic_store`, `seed_recaller`, `topic_searcher`,
`affective_recaller` — every adapter owns its own
`Option<&dyn EmbeddingProvider>` and computes
`provider.embed(query.text)` lazily on its hot path. There is **no
shared embedding_provider on PlanCollaborators**.

Two fix strategies:

### Strategy A (preferred — minimal change)

Add `embedding_provider: Option<&'a dyn EmbeddingProvider>` to
PlanCollaborators. Embed the query once at the top of `execute_plan`
(orchestrator.rs:949), pass `Option<&[f32]>` to `factual_to_scored`.
Compute cosine(query, memory_embedding) per candidate, emit as
`vector_score`.

Cost: 1 embed call per query (already paid by Associative/Affective
paths). For Hybrid where multiple sub-plans share the embedding,
cache at orchestrator entry. ~30-40 LoC.

### Strategy B (heavier — embedder owned by Loader)

Have `RecordLoader::load_embeddings` also return a query embedding,
or add `load_query_embedding(&str)` to the trait. More plumbing,
but cleaner if other plans later need it.

**Recommendation: Strategy A.** Smallest blast radius; matches how
Associative/Affective already work. Future plans can opt in by reading
the same field.

## Updated acceptance criteria

- [x] **AC-1**: Locate the ranker — `factual_to_scored` at
      orchestrator.rs:357.
- [x] **AC-2**: Hypothesis identified — H1+H3 fused (missing
      `vector_score` in Factual scoring stage; flat anchor-fraction
      `graph_score` provides no within-anchor-group ordering).
- [ ] **AC-3**: For each of the 9 ISS-161 SF queries, dump where the
      gold passage ranks in the Factual pool vs Hybrid path. Expect:
      gold IS in pool but ranked low (drowned by flat graph_score).
      (Skipped — not needed; root cause already confirmed by code.)
- [ ] **AC-4**: As AC-3 (skipped — same reason).
- [ ] **AC-5**: Ship Strategy A — add embedding_provider to
      PlanCollaborators, emit vector_score in factual_to_scored.
      ~40 LoC change. Unit tests + 1 integration assertion.
- [ ] **AC-6**: Re-run ISS-164 Phase 2 A/B sweep on conv-26 with the
      Factual ranker fix. Decision rule unchanged.

## Effort revised down

1.5–2 days. AC-1/AC-2 done now (~1h paper probe). AC-5 is a clean
~40-LoC fix + tests. AC-6 sweep ~1h wall.

## Why this didn't show up earlier (revised)

Same as before: Factual plan never received production traffic until
ISS-171 wired the classifier. Unit tests on `factual_to_scored`
assert score *consistency* and *ordering by graph_score*, but no test
asserts that semantically-related-but-non-lexical gold passages rank
in the top-K. That'd require an end-to-end retrieval test with real
embeddings. None exists. Worth filing as an ACO follow-up:
"Factual plan needs an integration test with semantic-relevance
gold passages."
