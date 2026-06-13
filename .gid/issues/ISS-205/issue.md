---
id: ISS-205
title: Temporal queries lose the dated episode to same-entity top-K crowding — ranking layer needs a date-bearing reservation
status: open
priority: P1
severity: retrieval-quality
tags:
- retrieval
- ranking
- temporal
- top-k
- factual-plan
- locomo
created: 2026-06-01
relates_to:
- ISS-190
- ISS-191
- ISS-201
- ISS-204
- .gid/issues/ISS-206/issue.md
- .gid/issues/ISS-208/issue.md
depends_on: ISS-204
---

# ISS-205: temporal queries lose the dated episode to same-entity top-K crowding

> **One-line:** ISS-204 made event dates first-class graph edges, so the
> dated episode is now *reachable* by traversal — but for high-degree
> anchors (Caroline = 31 dated episodes, Melanie = 18) the gold dated
> episode is *crowded out of the Factual-plan seed pool* by other episodes
> of the same entity. The fix is a ranking-layer reservation that
> guarantees date-bearing episodes survive top-K truncation. This is NOT
> an extraction-prompt change.

## Why this is the real fix (and PROMPT_V2 was not)

ISS-203's PROMPT_V2 (entity canonicalization) was the *candidate* lever for
the temporal multi-hop deficit. It was **falsified** via conv-44
cross-validation (see ISS-203, 2026-06-01 verdict):

- conv-26 single-hop +6.25pp **did not reproduce** — inverted to -6.67pp on conv-44
- conv-44 multi-hop -8.33pp **reproduced** conv-26's -8.11pp regression
- The explicit `belongs_to` / `associated_with` predicates V2 was supposed
  to emit produced **0 edges** in both arms (SQL-verified on
  `.tmpXHAFMU/substrate.db`) — the mechanism is inert
- V2's only measurable effect was a **-21% entity merge** (817 → 643 entity
  nodes), which *amplified* crowding rather than fixing it: fewer, denser
  anchors means more episodes per anchor competing for the same top-K slots

So the entity-canonicalization track is a dead end for temporal recall.
The crowding is a **ranking-layer** problem, and it must be fixed there.

## Evidence (SQL-verified on live AC-4 conv-26 DB)

DB: `/var/folders/48/.../.tmpcYbhzb/substrate.db` (post-ISS-204, 60
`occurred_on` edges, all with non-NULL `source_memory_id`).

### 1. The crowding distribution is heavily skewed

`occurred_on` edges per anchor entity:

| anchor | dated episodes |
|---|---|
| Caroline | 31 |
| Melanie | 18 |
| (all others) | 1 each |

13 distinct anchors hold 60 edges; two anchors hold 49 of them (82%). When a
query resolves to `Caroline`, the Factual plan seeds from her edges — but a
top-K of 10 cannot hold 31 candidate episodes. The *one* dated episode that
answers the query competes on a flat relevance score against 30 siblings and
routinely loses.

### 2. q0 is the canonical instance

conv-26 q0 (Caroline's LGBTQ support-group date, gold `2023-05-07`):
- the gold edge `Caroline --occurred_on--> 2023-05-07` (mem `83cd73d8`) is
  **present and traversable** (verified post-ISS-204)
- yet q0 still misses, because the gold episode does not survive into the
  top-K seed pool among Caroline's 31 dated episodes

ISS-204 made q0 *reachable*; it did not make q0 *retrieved*. That gap is this
issue.

### 3. conv-44 q11 is the concrete cross-corpus replica

conv-44 q11 (V2-on arm `.tmpXHAFMU`): the `2023-06-11` episode that answers
the query is present in the DB but was dropped from the top-K seed pool —
i.e. the same crowding mechanism reproduces on an independent corpus. The
date-bearing episode loses its slot to undated same-entity episodes.

## Proposed fix: date-bearing reservation in the Factual-plan seed pool

When the query is classified temporal (carries a date constraint or a
relative-time expression), the Factual plan's top-K truncation must
**reserve a quota of slots for date-bearing episodes** rather than letting a
flat relevance sort evict them.

Sketch (to be designed):
1. Detect that the query has a temporal intent (date literal, relative
   expression, "when"-type question).
2. When seeding from a high-degree anchor, partition candidate episodes into
   *date-bearing* (has an `occurred_on` edge / resolved temporal mark) vs
   *undated*.
3. Reserve `R` of the `K` seed slots for date-bearing episodes, ranked among
   themselves by relevance to the query's date constraint (interval overlap
   from ISS-191 AC-3 `temporal_score` is the natural ranker here).
4. Fill the remaining `K - R` slots with the normal relevance sort.

Open design questions:
- Is the reservation unconditional, or only when the resolved anchor exceeds
  a degree threshold (e.g. > K dated episodes)?
- Where in the pipeline does the reservation live — inside the Factual plan
  seeding, or as a reranker stage at the C.5 hook (like MMR / cross-encoder)?
  A reranker stage is more composable but only sees post-seed candidates; the
  problem is the *seed* pool truncation, so it likely belongs in seeding.
- How does `R` interact with non-temporal queries (must be a no-op / R=0)?
- How does the date-constraint ranker score "relative" expressions whose
  resolution lives in the temporal mark interval vs. a bare year?

## Acceptance criteria

- **AC-1** — Temporal-intent detection: a query carrying a date literal or
  relative-time expression is classified such that the reservation path
  activates; non-temporal queries take the unchanged path (byte-identical
  seed pool).
- **AC-2** — Reservation logic: for an anchor with > K dated episodes, the
  seed pool provably contains the top-`R` date-relevant episodes
  (interval-overlap ranked) even when a flat relevance sort would have
  evicted them. Unit test seeds a synthetic anchor with K+N dated episodes
  and asserts the gold-date episode survives.
- **AC-3** — q0 retrieval (RANKING ONLY): conv-26 q0's gold dated episode
  (Caroline support-group, gold `2023-05-07`) is provably lifted into the
  top-K seed pool with the reservation on, under the locked ISS-190 envelope
  (K=10, temp=0, HyDE/MMR/entity off, FACTUAL_REWEIGHT off, pipeline_pool=1,
  POPULATE off). Verified by the fused-pool probe: with the scoped
  reservation privilege the gold episode clears the top-10 cutoff on the
  graph axis (it already holds the pool's highest `vector_score`).
  **NOTE (2026-06-02):** the end-to-end q0 score flip 0→1 was RE-SCOPED to
  **ISS-206 AC-2** after the fused-pool probe (STAMP `20260602T024240Z`)
  proved the gold episode *is* retrieved into top-10 by this fix but still
  cannot be answered: its content string *"Caroline attended a LGBTQ support
  group"* carries **no in-text date** (the date lives only in the
  `occurred_on` edge / temporal mark, which the generator does not read).
  ISS-205 owns the ranking half; ISS-206 owns making the date legible to the
  generator. q0 flips only when BOTH land. This AC therefore proves the
  ranking lever works, not the end-to-end answer.
- **AC-4** — q11 cross-corpus: conv-44 q11 (`2023-06-11` episode) flips 0→1
  under the same envelope, proving the fix is corpus-general, not a conv-26
  artefact.
- **AC-5** — No regression: conv-26 + conv-44 overall and multi-hop within
  ±10% wobble vs the ISS-190 baseline with the reservation off; the
  single-fact / temporal categories show net non-negative movement.
- **AC-6** — Default gating: the reservation ships behind a serde-defaulted
  config knob (default off / inert) until the A/B clears AC-3..5, matching
  the ISS-139 MMR-default-off discipline.

## Design (2026-06-01, grounded in actual call sites)

Read of `crates/engramai/src/retrieval/plans/factual.rs` +
`crates/engramai/src/graph/store.rs` `GraphRead` trait settles the
seed-stage-vs-C.5-reranker question: **the fix belongs in seeding, not a
reranker.** A C.5 reranker (MMR / cross-encoder hook) only reorders
candidates that already survived into the pool — it can never re-admit an
episode that the seed truncation evicted. The crowding *is* the seed
truncation.

### The two existing seed paths in the Factual plan

1. **Edge-provenance seed (ISS-189 D1, factual.rs:566-592).** For every
   edge that Stage-2 traversal *returned*, the edge's `memory_id` is
   admitted to the candidate pool unconditionally. This already honors
   traversed edges.

2. **Recency-scan seed (factual.rs:~620-697).** For each anchor,
   `graph.memories_mentioning_entity(anchor, limit)` does a
   `ORDER BY recorded_at DESC LIMIT` scan. On a dense anchor (Caroline:
   ~188 mentions) this drops the answer episode — the inline ISS-189 note
   says "ranked 519/524 by recency on a dense anchor".

### Where ISS-205 intervenes

The gap is that path 1 only admits edges Stage-2 traversal *returned*, and
Stage-2 traversal is itself bounded (`anchors.truncate(max_anchors)` at
factual.rs:447, plus traversal depth/result caps). For a temporal query the
answer lives on an `occurred_on` edge whose source episode is being evicted
by the recency scan and whose edge is not guaranteed to be in the traversed
set.

**Fix: add a third seed path — a temporal edge seed — between paths 1 and 2.**

When the query carries temporal intent, for each resolved anchor:

1. `graph.edges_of(anchor, Some(&Predicate::occurred_on), false)` — pull
   ALL date-bearing edges for the anchor (this method is explicitly
   *uncapped* per its trait doc: "slot semantics requires the complete set").
   Mirror with `edges_into` for incoming date edges if needed.
2. Rank those edges by ISS-191 AC-3 `temporal_score` (interval overlap
   between the edge's date and the query's date constraint). Reuse that
   scorer — do NOT build a second one.
3. Force-admit the `source_memory_id` of the top-`R` edges into `memories`,
   exactly like the ISS-189 D1 loop does (`memories.entry(mid).or_default()`).
4. Tag them in `edge_seeded_ids` so `factual_to_scored` gives them the
   graph_score numerator privilege (ISS-192 fix 3), same as path 1.

This is additive and composes with the existing recency scan (which still
runs for neighborhood breadth). When temporal intent is absent, `R = 0` and
the path is a no-op → byte-identical seed pool (satisfies AC-1 / AC-6).

### Why `R` slots and not "admit all dated episodes"

Caroline has 31 dated episodes. Admitting all 31 would flood the pool and
starve the breadth scan. `R` is a small reservation (design default likely
3-5) of the *most date-relevant* episodes by interval overlap. The query
"when did X happen in May 2023?" reserves slots for episodes whose
`occurred_on` interval overlaps May 2023, not all 31.

### Temporal-intent detection (AC-1)

The query classifier already routes temporal queries (the conv-26 temporal
category exists). Reuse that signal rather than adding a parallel detector.
If the existing classification is too coarse (routes to Factual but doesn't
flag temporal intent within Factual), add a lightweight check: does the
query carry a date literal or relative-time expression (the same surface the
extractor's reference-date grounding keys on, ISS-190)? Resolve this during
implementation by reading the classifier output available to the Factual
plan.

### Config knob (AC-6)

`FactualPlanInputs` gains a serde-defaulted field (e.g.
`temporal_reservation: Option<usize>`, default `None` = off = `R=0`),
threaded from a `GraphQuery::with_temporal_reservation(R)` builder, mirroring
the `mmr_lambda_override` / `with_entity_channel` pattern. Default off until
A/B clears AC-3..5.

### Implementation order

1. Add `temporal_reservation` to `FactualPlanInputs` + `GraphQuery` builder
   (serde default off). Inert.
2. Add the temporal edge-seed path in factual.rs between the D1 seed and the
   recency scan, gated on `temporal_reservation.is_some()`. Unit test: seed a
   synthetic anchor with K+N dated episodes, assert gold-date episode
   survives (AC-2). Also assert byte-identity when off (AC-1).
3. Wire `ENGRAM_BENCH_TEMPORAL_RESERVATION` env in engram-bench (mirror the
   existing knob env vars).
4. A/B on conv-26 (AC-3: q0 gold lifted into top-K — flip gated on ISS-206)
   + conv-44 (AC-4: q11 flips — ranking-only, no ISS-206 dependency) under
   the locked ISS-190 envelope, regression gate AC-5.

## Notes

- This is downstream of ISS-204 (edge mechanism) and orthogonal to ISS-203
  (canonicalization, rejected). Do not couple it to the extraction prompt.
- The interval-overlap `temporal_score` from ISS-191 AC-3 is the natural
  ranker for the reserved slots — reuse it, do not build a second temporal
  scorer.
- Cross-year gaps (conv-44 q1/q26/q49, gold=2022 with 0 edges in 2022) are a
  *separate* defect — the dated episode does not exist as an edge at all.
  That is an extraction-coverage problem, not a ranking problem. Track
  separately; this issue assumes the gold edge exists.

## probe5 update (2026-06-02) — crowding premise REFINED + new anchor-miss bug

Decisive trace from probe5 (PID 67705, conv-26, R=5, ISS-190 envelope, DB
`.tmpK8lZyN`) with per-anchor/per-edge instrumentation in the reservation
loop. This **corrects** two assumptions in the original framing:

### 1. The gold episode is NOT crowded out of top-K

The fused-pool dump for `conv-26-q0` shows the gold episode
(`a838a102`, *"Caroline attended a LGBTQ support group"*) at **rank 6 of
217** — comfortably inside top-10 — with `vector_score=0.90` (pool's
highest), `score=0.53`, and `graph_score=0.2`. It is **not** evicted by
truncation. So the "31 siblings crowd the gold out of the seed pool" premise
does not hold for q0: the gold survives on vector strength alone. The
reservation's job here is not to *re-admit* an evicted episode; it is to
*lift its graph_score* (0.2 → reserved 0.7) so it ranks higher — and even
that turns out not to be the q0 blocker (see ISS-206).

This does NOT invalidate the reservation for genuinely high-degree evict
cases (conv-44 q11 may still be a true eviction — confirm in A/B), but the
conv-26 q0 instance is a *graph_score-not-lifted* case, not an *evicted*
case.

### 2. NEW BUG: the reservation never anchors on the query subject

For q0 *"When did Caroline go to the LGBTQ support group?"* the reservation
block entered correctly (`R=5 asks_for_date=true range=false anchors=5`) but
resolved its 5 anchors to: `Go`, `group`, `support`, `LGBTQ support group`,
`support group` — **all object/phrase fragments, every one with
`occurred_on_edges=0`**. The `Caroline` node (`d7f9a67a`, owner of all 31
dated `occurred_on` edges incl the gold) was **NOT among the resolved
anchors**. So `edges_of(Caroline, OccurredOn)` was never called and the
reserved privilege never fired for the gold episode — its graph_score stayed
0.2.

The Melanie queries show the same shape: they resolve to the `Melanie` node
but `edges_of` returns only **1** of Melanie's 17 dated edges at query time
(the most-recent by `recorded_at`), even though the worker-pool drain
reported `jobs_in_flight: 0, jobs_processed: 456`. The 17 edges' `recorded_at`
span a 722s window — i.e. canonical-node edge population is still settling
well past the drain-complete marker. **This is a latent edge-visibility /
drain-accounting concern** (the drain decrements in-flight after
`process()` returns incl `insert_edge` commit, so the accounting looks
correct, yet the query sees an undercount). Filed as a follow-up (see
ISS-207 / new issue) — it is NOT the q0 blocker but it weakens the
reservation on dense anchors generally.

### Net effect on ISS-205 scope

- AC-3 (q0) remains correctly re-scoped to ISS-206 (date legibility) — gold
  is already retrieved; ranking is not the q0 gate.
- The **anchor-resolution miss** must be fixed for the reservation to work at
  all on subject-of-"when" queries: the reservation needs to resolve and
  anchor on the query's *subject entity* (Caroline), not only the object
  phrases. This is a prerequisite for AC-2/AC-4 to mean anything.
- The **edge-visibility undercount on dense anchors** (Melanie 1-of-17 at
  query time) is a separate reliability bug that can silently starve the
  reservation; track it explicitly before relying on the reservation in
  production.

### Cleanup

The `[ISS205-PROBE]` eprintlns added to `factual.rs` for this trace were
reverted (`git checkout`); the engramai tree is clean.

---

## Anchor-resolution miss FIXED (2026-06-02)

### Root cause (proven, not reasoned)

Built `crates/engramai/examples/iss205_anchor_probe.rs` to run the REAL
`GraphEntityResolver::resolve()` against the forensic DB
(`.tmpK8lZyN/substrate.db`) on the live q0 query string. Result before fix:

```
resolver returned 6 anchor(s):
  [ 0] strength=0.7000  Go
  [ 1] strength=0.7000  group
  [ 2] strength=0.7000  support
  [ 3] strength=0.7000  LGBTQ support group
  [ 4] strength=0.7000  support group
  [ 5] strength=0.7000  Caroline  <== BELOW max_anchors=5
```

Every alias hit tied at **exactly 0.7000**. The resolver computes
`match_strength = alias_boost(0.7) + 0.3*recency_score`, but each mention is
an **isolated** `search_candidates` call returning a single alias hit. With a
one-element candidate set, `min_last_seen == max_last_seen`, so
`recency_score = (ls - ls)/span = 0.0` — *always*. Recency could never break
ties. The final order fell to the resolver's deterministic secondary sort
`entity_id ASC`, an arbitrary UUID ordering. Caroline's UUID `d7f9a67a` sorts
last among the six, so the SUBJECT entity that owns all 31 `occurred_on`
edges (incl gold) was truncated below `max_anchors=5`, while five
object-phrase fragments owning **zero** edges survived. Pure UUID accident.

This was NOT entity fragmentation (one Caroline canonical confirmed), NOT a
missing alias (`caroline → d7f9a67a` confirmed), NOT a missing
`graph_entities`/`nodes` row (both present).

### Fix

`graph_entity_resolver.rs::resolve()` now collects raw candidates carrying
their projected `last_seen`, dedupes by `entity_id`, then **recomputes
recency across the merged deduped pool** before the final sort. This is the
cross-pool tiebreak the original doc-comment always promised but the per-call
computation could never deliver. Single-candidate pools have a zero span →
recency 0.0 → score collapses to the alias-only 0.7 (no divide-by-zero, no
spurious boost).

After fix (same probe, same DB):

```
resolver returned 6 anchor(s):
  [ 0] strength=1.0000  Caroline   <== now rank 0 (newest last_seen)
  [ 1] strength=0.9984  support
  [ 2] strength=0.8861  Go
  [ 3] strength=0.8474  group
  [ 4] strength=0.7027  support group
  [ 5] strength=0.7000  LGBTQ support group
```

Caroline rises to index 0 (full recency: 0.7 + 0.3*1.0 = 1.0), comfortably
inside `max_anchors=5`. The reservation will now anchor on Caroline and
`edges_of(Caroline, OccurredOn)` returns all 31 edges (DB-verified), so the
date-asking branch admits the earliest (gold) episode.

### Tests

- 2 new regression tests in `graph_entity_resolver.rs`:
  - `iss205_recency_breaks_ties_subject_ranks_first` — six exact alias hits,
    asserts the most-recently-touched subject ranks first with strength ~1.0
    and a strict gap at the top (proving recency separated the otherwise-tied
    hits, not a UUID sort).
  - `iss205_single_alias_hit_collapses_to_alias_only_score` — degenerate
    one-candidate pool scores exactly 0.7 (zero-span guard).
- 19/19 resolver tests pass; full engramai lib suite 2113/2113 pass.

### Remaining

This unblocks the reservation anchoring. AC-3 (q0 end-to-end) still depends on
ISS-206 (date legibility) — gold has no in-text date — and a controlled
ranking A/B should be run on a date-BEARING query after ISS-208
(edge-visibility undercount on dense anchors) is fixed, to avoid a confounded
measurement.

## Baseline crowding — DB-verified (V2-OFF default envelope, 2026-06-13)

The original follow-up asked for the crowding claim to be DB-verified
*before* trusting the design. Done, on the AC-3 fresh conv-26 substrate
`.tmppGptnQ` (locked ISS-190 envelope: V2 OFF, entity-set OFF, K=10 — so
this is a property of the BASE graph, not of any rejected lever):

- **Dated episodes are rare:** 53 / 440 memories (~12%) carry a
  `day`-precision temporal mark + `occurred_on` edge.
- **Entity graph is highly concentrated:** out-degree `Caroline`=197,
  `Melanie`=78, long tail (art=33, painting=23, family=13, …).
- **Per-anchor haystack is huge:** `Melanie` is mentioned by **136**
  distinct memories; `Caroline`=197 out-edges. A single dated episode is
  one needle in a 100–200-memory same-anchor haystack → the recency-scan
  seed path (factual.rs ~620) evicts it exactly as the design predicts.
- **Recall-probe corroboration (ISS-225, 2026-06-13):** conv-26 temporal
  recall@200 ≈ 0.47 (gold IS in the pool) while end-to-end temporal score
  is far lower — gold is *present but ranked below top-K*, the textbook
  crowding signature this issue targets.

This confirms ISS-205's reservation is an **independent** retrieval lever:
the crowding exists in the baseline without ISS-203 V2. The reservation is
therefore valuable on its own (temporal/single-hop), AND is the
trade-off-breaker that would let ISS-203 V2 entity-consolidation ship
without regressing multi-hop (consolidation raises density → crowding →
which reservation neutralises for the dated-episode case).

## Cross-issue standing (2026-06-13, from ISS-201 lever sweep closure)

ISS-201 closed out all ranking levers (K/MMR/CE/HyDE/entity-channel/
factual-reweight: none ships) and lever-(b) entity-set synthesis (falsified
for ship: single-hop list net -3; input completeness blocked by entity
fragmentation; the de-frag fix ISS-203 V2 is itself rejected for crowding).
The converged conclusion: the list/aggregate + temporal deficit is a
multi-objective tension (completeness vs crowding), and **ISS-205 is the
highest-leverage open item** because it is the only mechanism that relaxes
that tension. Recommended as the next implementation target.

Implementation is fully designed above and the GraphQuery surface already
exists (`temporal_reservation_override`, default-off/inert). Remaining work
is: the third seed path in factual.rs + unit tests (AC-1/AC-2) + the
`ENGRAM_BENCH_TEMPORAL_RESERVATION` wire + conv-26/conv-44 A/B (AC-3..5).
Note the q0 end-to-end flip also needs ISS-206 (date legibility to the
generator); ISS-205 alone proves the ranking half (gold reaches top-K).
